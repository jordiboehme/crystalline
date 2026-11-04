/**
 * The frame's own behavior: the domain list it fetches, where the search box
 * sends you, what the theme control writes, what logging out does, and what
 * the sidebar becomes once a domain is open.
 */

import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect } from "react";
import { MemoryRouter, useNavigate } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
import { ApiProblem, api, setCsrfToken } from "../api/client";
import { FOCUS_HEADING } from "../focusHeading";
import { defined } from "../test/assert";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);
const setCsrfTokenMock = vi.mocked(setCsrfToken);

function serve(routes: Record<string, (path: string) => unknown>) {
  apiMock.mockImplementation(answersFor(routes));
}

/** Signed in, with one domain to list and a quiet activity feed. */
function serveSignedIn(extra: Record<string, (path: string) => unknown> = {}) {
  serve({
    "/auth/me": () => meResponse({ user: userFixture() }),
    "/domains": domainsResponse,
    // The home screen behind this frame reads the feed; an unstubbed route
    // would fail and put a second alert on screen.
    "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    ...extra,
  });
}

/** Two domains, so the switcher has somewhere to switch to. */
function twoDomainsResponse() {
  return {
    behavior: ["Search before answering from memory."],
    domains: [
      {
        name: "eng",
        kind: "file",
        engrams: 4,
        when_to_use: ["Route here for eng questions."],
      },
      { name: "ops", kind: "file", engrams: 2, when_to_use: [] },
    ],
  };
}

/**
 * The browse payload, answering with the folder that was asked for.
 *
 * Every row carries a `status`, which is the shape the endpoint answers with:
 * a browse row is what a tree is drawn from, so it says what state its engram
 * is in as well as where it lives. That is what the fade below is reading.
 */
function treeResponse(path: string) {
  if (path.includes("path=notes")) {
    return {
      domain: "eng",
      path: "notes",
      folders: [],
      engrams: [
        {
          permalink: "notes/beta",
          title: "Beta",
          type: "engram",
          status: "stable",
          path: "notes/beta.md",
        },
      ],
    };
  }
  return {
    domain: "eng",
    path: "/",
    folders: ["notes"],
    folder_counts: { notes: 12 },
    engrams: [
      // The engine lists the MANIFEST as an engram of the domain like any
      // other, which is what the sidebar's pinned row above the tree would
      // otherwise say twice.
      {
        permalink: "manifest",
        title: "MANIFEST",
        type: "manifest",
        status: "current",
        path: "MANIFEST.md",
      },
      {
        permalink: "alpha",
        title: "Alpha",
        type: "engram",
        status: "stable",
        path: "alpha.md",
      },
      {
        permalink: "old",
        title: "Old Way",
        type: "decision",
        status: "deprecated",
        path: "old.md",
      },
    ],
  };
}

/** Signed in, inside a domain: the frame's own reads plus the screen's. */
function serveInDomain(extra: Record<string, (path: string) => unknown> = {}) {
  serve({
    "/auth/me": () => meResponse({ user: userFixture() }),
    "/domains": twoDomainsResponse,
    "/domains/eng/tree": treeResponse,
    "/domains/eng/manifest": () => ({ domain: "eng", markdown: "" }),
    // The domain screen behind the frame pages this, scoped to the folder it
    // is browsing: the tree is navigation, the listing is what is in there.
    "/domains/eng/engrams": (path) =>
      path.includes("path=notes")
        ? {
            mode: "text",
            total: 1,
            page: 1,
            limit: 50,
            count: 1,
            hits: [
              {
                domain: "eng",
                permalink: "notes/beta",
                title: "Beta",
                engram_type: "engram",
                kind: "engram",
                status: "stable",
                tags: [],
              },
            ],
          }
        : { mode: "text", total: 0, page: 1, limit: 50, count: 0, hits: [] },
    "/domains/eng/engrams/alpha": () => ({
      domain: "eng",
      permalink: "alpha",
      title: "Alpha",
      url: "crystalline://eng/alpha",
      content: "# Alpha\n",
      frontmatter: {},
      observations: [],
      relations: [],
      links: [],
    }),
    "/domains/eng/engrams/notes/beta": () => ({
      domain: "eng",
      permalink: "notes/beta",
      title: "Beta",
      url: "crystalline://eng/notes/beta",
      content: "# Beta\n",
      frontmatter: {},
      observations: [],
      relations: [],
      links: [],
    }),
    "/graph": () => ({ nodes: [], edges: [], truncated: false }),
    "/vocabulary": () => ({ domain: "eng", tags: [] }),
    ...extra,
  });
}

/** The sidebar, whichever mode it is in. */
async function sidebar(): Promise<HTMLElement> {
  return screen.findByRole("navigation", { name: /^Domain/ });
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/**
 * A window narrower than the medium breakpoint.
 *
 * jsdom answers no media queries at all - it ships no `matchMedia` - so a
 * frame asking about the viewport is told, by default, that it is on the wide
 * side, and a test that wants a phone says so by installing an API that
 * answers no to everything: narrow, and light while it is at it, which is what
 * the theme provider asks the same call.
 */
function stubNarrowWindow() {
  // On `window` itself rather than through `vi.stubGlobal`: this environment
  // keeps jsdom's window beside the Node globals rather than as them (see
  // `test/setup.ts`), and the frame asks `window`.
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }),
  });
}

/**
 * A window whose width can change under the frame: starts narrow, and
 * `resize(true)` takes it past the medium breakpoint (or back).
 */
function stubResizableWindow(): (wide: boolean) => void {
  let wide = false;
  const listeners = new Set<(event: { matches: boolean }) => void>();
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (query: string) => ({
      get matches() {
        return query.includes("min-width") ? wide : false;
      },
      media: query,
      addEventListener: (
        _: string,
        fn: (event: { matches: boolean }) => void,
      ) => {
        if (query.includes("min-width")) {
          listeners.add(fn);
        }
      },
      removeEventListener: (
        _: string,
        fn: (event: { matches: boolean }) => void,
      ) => {
        listeners.delete(fn);
      },
    }),
  });
  return (next) => {
    act(() => {
      wide = next;
      listeners.forEach((fn) => {
        fn({ matches: wide });
      });
    });
  };
}

beforeEach(() => {
  apiMock.mockReset();
  setCsrfTokenMock.mockReset();
  document.documentElement.removeAttribute("data-theme");
  Reflect.deleteProperty(window, "matchMedia");
  // The sidebar's own width is remembered across sessions, so each test
  // starts from a browser that has never been told anything.
  localStorage.clear();
});

/** Open the avatar menu, signed in as the fixture's person. */
async function openAvatar(
  user: ReturnType<typeof userEvent.setup>,
  name = "Ada Lovelace",
): Promise<HTMLElement> {
  await user.click(await screen.findByRole("button", { name }));
  // The body is a lazy part, so the first row is the sign it has arrived.
  await screen.findByRole("menuitem", { name: "Maintenance" });
  return screen.getByRole("menu");
}

describe("the layout", () => {
  it("moves focus to the main region on navigation", async () => {
    // /auth/me + /domains + a domain tree/engrams, the way the sidebar and
    // the screen behind it both need it.
    serveInDomain();

    renderApp("/");
    // Scoped to the sidebar: the same domain is also named by a card on the
    // home screen behind it, and an unscoped query would match both.
    const domains = await screen.findByRole("navigation", { name: "Domains" });
    const domainLink = await within(domains).findByRole("link", {
      name: /^eng/,
    });
    await userEvent.click(domainLink);

    await waitFor(() => {
      expect(document.activeElement).toBe(screen.getByRole("main"));
    });
  });

  it("leaves focus to the screen when a navigation asks to focus its heading", async () => {
    serveInDomain();
    let go: (to: string, state: unknown) => void = () => undefined;
    function Capture() {
      const navigate = useNavigate();
      useEffect(() => {
        go = (to, state) => void navigate(to, { state });
      }, [navigate]);
      return null;
    }
    render(
      <MemoryRouter initialEntries={["/"]}>
        <App />
        <Capture />
      </MemoryRouter>,
    );
    await screen.findByRole("navigation", { name: "Domains" });
    act(() => {
      go("/d/eng", FOCUS_HEADING);
    });
    await screen.findByRole("heading", { name: "eng", level: 1 });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(document.activeElement).not.toBe(screen.getByRole("main"));
  });

  it("lists the instance's domains in the sidebar", async () => {
    serveSignedIn();

    renderApp("/");

    const domains = await screen.findByRole("navigation", { name: "Domains" });
    const link = await within(domains).findByRole("link", { name: /eng/ });
    expect(link).toHaveAttribute("href", "/d/eng");
    // Outside a domain there is nothing to switch between yet: the flat list
    // is the whole sidebar, and it stays that way.
    expect(screen.queryByRole("button", { name: /^Domain:/ })).toBeNull();
  });

  it("does not call the instance empty when a contributor has Reflection-only domains", async () => {
    serveSignedIn({
      "/domains": () => ({ behavior: [], domains: [] }),
      "/reflection/domains": () => ({
        domains: [
          {
            domain: "eng",
            state: "on",
            right: "edit",
            engrams_visible: false,
            home: "here",
            home_url: null,
            open_threads: 1,
          },
        ],
      }),
    });

    renderApp("/");

    const domains = await screen.findByRole("navigation", { name: "Domains" });
    // Home draws the Reflection section from the same listing the sidebar
    // reads, so once it is there the sentence's absence means something.
    await screen.findByRole("region", { name: "Reflection" });
    expect(
      within(domains).queryByText(
        "No domains are registered on this instance yet.",
      ),
    ).toBeNull();
  });

  it("badges a private domain in the sidebar and leaves a shared one plain", async () => {
    serveSignedIn({
      "/domains": () => ({
        behavior: [],
        domains: [
          { name: "eng", kind: "file", engrams: 4, when_to_use: [] },
          {
            name: "lab",
            kind: "file",
            engrams: 2,
            private: true,
            when_to_use: [],
          },
        ],
      }),
    });

    renderApp("/");

    const domains = await screen.findByRole("navigation", { name: "Domains" });
    const lab = await within(domains).findByRole("link", { name: /^lab/ });
    expect(lab).toHaveTextContent("private");
    // The badge rides on the listing's own field, so a row that does not say
    // it is private wears nothing - the same as a row from a server that
    // never heard of the field.
    expect(
      within(domains).getByRole("link", { name: /^eng/ }),
    ).not.toHaveTextContent("private");
  });

  it("says what went wrong instead of emptying the sidebar", async () => {
    serveSignedIn({
      "/domains": () => {
        throw new ApiProblem(403, "forbidden", "this account is a viewer");
      },
    });

    renderApp("/");

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "this account is a viewer",
    );
    // The sidebar's list ends in the error state: no loader left spinning and
    // no claim that the instance is empty.
    const list = await sidebar();
    expect(within(list).getByRole("alert")).toHaveTextContent(
      "this account is a viewer",
    );
    expect(
      within(await screen.findByRole("main")).queryByRole("alert"),
    ).toBeNull();
    expect(
      within(list).queryByRole("status", { name: "Loading domains" }),
    ).toBeNull();
    expect(screen.queryByText(/No domains are registered/)).toBeNull();
    // A refusal is rendered where it happened, never as a bounce to the login
    // screen the caller is already past.
    expect(screen.queryByLabelText("Password")).not.toBeInTheDocument();
  });

  it("leaves the failed listing to the sidebar's alert on a screen other than Home", async () => {
    serveSignedIn({
      "/domains": () => {
        throw new ApiProblem(403, "forbidden", "this account is a viewer");
      },
      "/graph": () => ({ nodes: [], edges: [], truncated: false }),
    });

    renderApp("/graph");

    const list = await sidebar();
    expect(await within(list).findByRole("alert")).toHaveTextContent(
      "this account is a viewer",
    );
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });

  it("routes the search box to the search screen", async () => {
    // The screen it lands on runs the query it was handed, so the routes it
    // needs are stubbed here: an unstubbed one is a failed request, and the
    // reader would arrive at their results behind an error box.
    serveSignedIn({
      "/search": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({ tags: [] }),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Search"), "salience{Enter}");

    expect(
      await screen.findByRole("heading", { name: "Search" }),
    ).toBeVisible();
    // A clean landing: the query ran, and nothing on the way there failed.
    await waitFor(() => {
      expect(screen.getByText(/no engram matches/i)).toBeVisible();
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("names every control in the top bar, icon-shaped ones included", async () => {
    // An administrator, because the graph icon is the one control the frame
    // only offers to a session that may use it.
    serve({
      "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
      // GitHub is off on this instance, which is what keeps the share action
      // out of the row below; `ShareAction.test.tsx` is where it is drawn.
      "/settings/github": () => ({ enabled: false, connected: false }),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    // The icons carry no text, so the name is the only thing a reader
    // arriving by keyboard or by screen reader has to go on.
    expect(screen.getByRole("button", { name: "Domains" })).toHaveAttribute(
      "aria-controls",
      "domain-sidebar",
    );
    // GitHub settings are reached through the identity menu's Settings item;
    // the top bar carries no separate door to the same screen.
    expect(screen.queryByRole("link", { name: "GitHub" })).toBeNull();
    // And nothing to share on an instance with no origins to share to.
    expect(screen.queryByRole("button", { name: "Share changes" })).toBeNull();
    // The old strip of Maintenance, width, text size, theme and help controls
    // is gone: all of it lives in the avatar menu now.
    for (const gone of [
      /^Theme:/,
      "Help",
      "Use full width",
      "Use large text",
    ]) {
      expect(screen.queryByRole("button", { name: gone })).toBeNull();
    }
    expect(screen.queryByRole("link", { name: "Maintenance" })).toBeNull();
    expect(screen.getByRole("button", { name: "Ada Lovelace" })).toBeVisible();
    expect(screen.getByRole("link", { name: "Fluid" })).toHaveAttribute(
      "href",
      "/",
    );
    // Who you are is one item, not a strip: the accounts screen is reached
    // from inside it rather than from an icon of its own beside it.
    expect(screen.queryByRole("link", { name: "Users" })).toBeNull();

    // And the pointer is told the same names the screen reader is, by the
    // app's own tooltip rather than the browser's - so none of these
    // carries a `title` for a second label to be drawn from.
    for (const control of [
      screen.getByRole("button", { name: "Domains" }),
      screen.getByRole("button", { name: "Ada Lovelace" }),
    ]) {
      expect(control).not.toHaveAttribute("title");
    }
    const user = userEvent.setup();
    await user.hover(screen.getByRole("button", { name: "Domains" }));
    expect(
      await screen.findByRole("tooltip", {}, { timeout: 2000 }),
    ).toHaveTextContent("Domains");
  });

  it("gathers who you are and what only you can reach into one menu", async () => {
    serve({
      "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const user = userEvent.setup();
    await openAvatar(user);

    // The header says who, in the account's own name and its role in words,
    // and is not something to press - it is what the rest of the menu is about.
    expect(
      await screen.findByText("Admin on this instance", { exact: false }),
    ).toBeVisible();
    expect(
      screen.queryByRole("menuitem", { name: /Admin on this instance/ }),
    ).toBeNull();
    expect(screen.getByRole("menuitem", { name: "Profile" })).toHaveAttribute(
      "href",
      "/profile",
    );
    expect(
      screen.getByRole("menuitem", { name: "Maintenance" }),
    ).toHaveAttribute("href", "/maintenance");
    // Then the two screens this session may reach, then the way out.
    expect(
      await screen.findByRole("menuitem", { name: "Users" }),
    ).toHaveAttribute("href", "/users");
    expect(screen.getByRole("menuitem", { name: "Settings" })).toHaveAttribute(
      "href",
      "/settings/github",
    );
    expect(screen.getByRole("menuitem", { name: "Log out" })).toBeVisible();
  });

  it("offers no accounts row to a session that may not administer", async () => {
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const user = userEvent.setup();
    await openAvatar(user);

    // The editor's own identity, and the way out. The screens behind Users and
    // Settings both refuse anyone but an admin, and the frame offers no door
    // that will not open.
    expect(
      await screen.findByText("Editor on this instance", { exact: false }),
    ).toBeVisible();
    expect(screen.getByRole("menuitem", { name: "Maintenance" })).toBeVisible();
    expect(screen.getByRole("menuitem", { name: "Log out" })).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Users" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Settings" })).toBeNull();
  });

  it("writes the chosen theme onto the document", async () => {
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const user = userEvent.setup();
    await openAvatar(user);
    await user.click(screen.getByRole("menuitemradio", { name: "Dark" }));

    await waitFor(() => {
      expect(document.documentElement.dataset.theme).toBe("dark");
    });
  });

  it("ends the session and asks who you are again", async () => {
    let signedIn = true;
    serve({
      "/auth/me": () =>
        signedIn
          ? meResponse({ user: userFixture(), csrf: "sess" })
          : meResponse(),
      "/auth/logout": () => {
        signedIn = false;
        return { ok: true };
      },
      "/domains": domainsResponse,
    });

    renderApp("/");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Ada Lovelace" }),
    );
    await user.click(await screen.findByRole("menuitem", { name: "Log out" }));

    expect(await screen.findByLabelText("Name")).toBeVisible();

    // The screen says the session is over; this says the token went with it.
    // Asserted against the clock rather than as a bare "was called with null",
    // because the re-probe that follows a logout answers with a null token
    // too and would satisfy that on its own. What has to be true is the
    // order: the token is dropped as part of logging out, before any further
    // request goes out, so nothing in between can carry a dead one.
    expect(setCsrfTokenMock).toHaveBeenCalledWith(null);
    const dropped = setCsrfTokenMock.mock.calls.findIndex(
      ([token]) => token === null,
    );
    const droppedAt = setCsrfTokenMock.mock.invocationCallOrder[dropped];
    const probes = apiMock.mock.calls
      .map((call, index) => ({
        path: call[0],
        order: defined(
          apiMock.mock.invocationCallOrder[index],
          "the call's invocation order",
        ),
      }))
      .filter((call) => call.path === "/auth/me");
    const lastProbe = defined(probes[probes.length - 1], "the last probe");
    expect(droppedAt).toBeLessThan(lastProbe.order);
  });
});

/**
 * Inside a domain the sidebar stops being a list of everything and becomes the
 * way around one thing: which domain you are in, and what is in it. A domain
 * is a place you work in rather than an entry you picked once, so the way out
 * and the way across stay on screen while you are in it.
 */
describe("the sidebar inside a domain", () => {
  it("switches from the domain list to the domain's own navigation", async () => {
    serveInDomain();

    renderApp("/d/eng");

    const nav = await screen.findByRole("navigation", { name: "Domain eng" });
    // Overview goes to the domain's front page, and it is marked current
    // because that page is what is open. The domain's name is not repeated:
    // the bar's switcher names it.
    const row = await within(nav).findByRole("link", { name: "Overview" });
    expect(row).toHaveAttribute("href", "/d/eng/engrams");
    expect(row).toHaveAttribute("aria-current", "page");
    expect(within(nav).queryByRole("link", { name: "eng" })).toBeNull();
    expect(
      within(nav).getByRole("heading", { level: 2, name: "Engrams" }),
    ).toBeVisible();
    // The way across and the way back out are the top bar's switcher now,
    // so the sidebar no longer carries either.
    expect(
      within(nav).queryByRole("button", { name: /switch domain/i }),
    ).toBeNull();
    expect(within(nav).queryByRole("link", { name: "All domains" })).toBeNull();
    expect(
      screen.getByRole("button", { name: "eng, switch domain" }),
    ).toBeVisible();
    // And below it, what this domain holds: its folders and its engrams.
    expect(
      await within(nav).findByRole("button", { name: "Expand notes" }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(within(nav).getByRole("link", { name: "Alpha" })).toHaveAttribute(
      "href",
      "/d/eng/e/alpha",
    );
    // The flat list is gone: two lists of domains at once would be two answers
    // to the same question.
    expect(within(nav).queryByRole("link", { name: /^ops/ })).toBeNull();
    // No dropdown wearing the domain's name, and no pinned MANIFEST row: the
    // MANIFEST is read on the domain page the row already links to.
    expect(within(nav).queryByRole("button", { name: /^Domain:/ })).toBeNull();
    expect(within(nav).queryByRole("link", { name: "MANIFEST" })).toBeNull();
  });

  it("draws each folder's engram count beside its name, kept out of the link's name", async () => {
    serveInDomain();

    renderApp("/d/eng");

    const nav = await screen.findByRole("navigation", { name: "Domain eng" });
    const folder = await within(nav).findByRole("link", { name: "notes" });
    expect(folder).toHaveTextContent("notes12");
    expect(within(folder).getByText("12")).toHaveAttribute(
      "title",
      "12 engrams",
    );
    expect(within(nav).getByText("12 engrams")).toHaveClass("sr-only");
  });

  it("draws no count for a folder a server sends none for", async () => {
    serveInDomain({
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: ["notes"],
        engrams: [],
      }),
    });

    renderApp("/d/eng");

    const nav = await screen.findByRole("navigation", { name: "Domain eng" });
    const folder = await within(nav).findByRole("link", { name: "notes" });
    expect(folder).toHaveTextContent(/^notes$/);
    expect(within(nav).queryByText(/engrams?$/)).toBeNull();
  });

  it("moves to another domain when the switcher picks one", async () => {
    serveInDomain({
      "/domains/ops/tree": () => ({
        domain: "ops",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/domains/ops/manifest": () => ({ domain: "ops", markdown: "" }),
      "/domains/ops/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
    });

    renderApp("/d/eng");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "eng, switch domain" }),
    );

    // Every domain is offered with what it holds, which is what makes the
    // choice between them a choice rather than a guess.
    const other = await screen.findByRole("menuitem", { name: /^ops/ });
    expect(other).toHaveTextContent("2");
    await user.click(other);

    expect(
      await screen.findByRole("heading", { level: 1, name: "ops" }),
    ).toBeVisible();
    const nav = await screen.findByRole("navigation", { name: "Domain ops" });
    expect(
      await within(nav).findByRole("link", { name: "Overview" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("badges a private domain in the switcher", async () => {
    // The sidebar has two forms and the same domain has to read the same way
    // in both: the flat list outside a domain, and this switcher inside one.
    serveInDomain({
      "/domains": () => ({
        behavior: [],
        domains: [
          { name: "eng", kind: "file", engrams: 4, when_to_use: [] },
          {
            name: "ops",
            kind: "file",
            engrams: 2,
            private: true,
            when_to_use: [],
          },
        ],
      }),
    });

    renderApp("/d/eng");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "eng, switch domain" }),
    );

    expect(
      await screen.findByRole("menuitem", { name: /^ops/ }),
    ).toHaveTextContent("private");
    expect(
      screen.getByRole("menuitem", { name: /^eng/ }),
    ).not.toHaveTextContent("private");
  });

  it("splits a folder row: the name browses it, the icon opens it here", async () => {
    serveInDomain();

    renderApp("/d/eng");
    const nav = await sidebar();

    // Two controls, side by side rather than one inside the other: a link
    // nested in a button is an accessibility violation and a guess about
    // which of the two a click meant.
    const name = await within(nav).findByRole("link", { name: "notes" });
    expect(name).toHaveAttribute("href", "/d/eng/engrams?path=notes");
    const toggle = within(nav).getByRole("button", { name: "Expand notes" });
    expect(toggle.contains(name)).toBe(false);
    expect(name.contains(toggle)).toBe(false);

    // The icon looks inside without moving the screen beside it, and says
    // which of the two things it does next.
    await userEvent.click(toggle);
    expect(
      await within(nav).findByRole("button", { name: "Collapse notes" }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(
      await within(nav).findByRole("link", { name: "Beta" }),
    ).toBeVisible();
  });

  it("draws one folder glyph per row, on the control that opens it", async () => {
    serveInDomain();

    renderApp("/d/eng");
    const nav = await sidebar();

    // The chevron beside the folder icon read as a dot rather than a control,
    // so the two became one: the icon IS the toggle now, and its open face is
    // what the chevron used to say.
    const toggle = await within(nav).findByRole("button", {
      name: "Expand notes",
    });
    const name = within(nav).getByRole("link", { name: "notes" });
    expect(toggle.querySelectorAll("svg")).toHaveLength(1);
    expect(toggle.querySelector("svg")?.getAttribute("class")).toContain(
      "lucide-folder",
    );
    expect(name.querySelectorAll("svg")).toHaveLength(0);
    expect(toggle).not.toHaveTextContent(/\S/);

    // Open, and the same one glyph says so.
    await userEvent.click(toggle);
    const opened = await within(nav).findByRole("button", {
      name: "Collapse notes",
    });
    expect(opened.querySelectorAll("svg")).toHaveLength(1);
    expect(opened.querySelector("svg")?.getAttribute("class")).toContain(
      "lucide-folder-open",
    );
  });

  it("offers the whole of a folder too big to draw, without quoting a count", async () => {
    serveInDomain({
      "/domains/eng/tree": (path) =>
        path.includes("path=notes")
          ? {
              domain: "eng",
              path: "notes",
              folders: [],
              engrams: [
                {
                  permalink: "notes/beta",
                  title: "Beta",
                  type: "engram",
                  status: "stable",
                  path: "notes/beta.md",
                },
              ],
              truncated: true,
              total: 620,
            }
          : { ...treeResponse(path), truncated: true, total: 500 },
    });

    renderApp("/d/eng");
    const nav = await sidebar();

    // The root level was cut too, and the row that says so names the domain
    // rather than a folder, because the root is not a folder somebody opened.
    const whole = await within(nav).findByRole("link", {
      name: "Browse all engrams in this domain",
    });
    expect(whole).toHaveAttribute("href", "/d/eng/engrams");

    await userEvent.click(
      await within(nav).findByRole("button", { name: "Expand notes" }),
    );

    // The sidebar never tries to show a big folder whole: it hands the folder
    // to the screen, which pages it. No number: this is a link, so its words
    // are a promise about where it goes, and the count this level knows is the
    // engrams directly in it while the screen counts everything below it too.
    const all = await within(nav).findByRole("link", {
      name: "Browse all of notes",
    });
    expect(all).toHaveAttribute("href", "/d/eng/engrams?path=notes");
    expect(within(nav).queryByText(/620/)).toBeNull();
  });

  it("marks the folder being browsed and opens it", async () => {
    serveInDomain();

    renderApp("/d/eng?path=notes");
    const nav = await sidebar();

    // Browsing a folder is being somewhere, so the tree says where: the same
    // mark an engram row wears, on the row that is the place.
    const name = await within(nav).findByRole("link", { name: "notes" });
    expect(name).toHaveAttribute("aria-current", "page");
    expect(
      within(nav).getByRole("button", { name: "Collapse notes" }),
    ).toHaveAttribute("aria-expanded", "true");
  });

  it("stops calling a folder current once a filter is on", async () => {
    serveInDomain();

    // Browsing `notes`, then filtering: the screen leaves the folder and
    // lists the whole domain, and it says so above the rows.
    renderApp("/d/eng?path=notes&tags=eng");
    const nav = await sidebar();
    expect(
      await screen.findByText(/Filtered across the whole domain/),
    ).toBeVisible();

    // So the tree does not claim the reader is in that folder. The mark would
    // be a plain misstatement: it names a page the reader is not on, and the
    // link under it drops the filter rather than staying put.
    const name = await within(nav).findByRole("link", { name: "notes" });
    expect(name).not.toHaveAttribute("aria-current");
    // The branch stays open, though: the filter is a lens over the domain,
    // not a reason to fold away where the reader just was.
    expect(
      within(nav).getByRole("button", { name: "Collapse notes" }),
    ).toHaveAttribute("aria-expanded", "true");
  });

  it("asks for a folder only when it is opened", async () => {
    serveInDomain();

    renderApp("/d/eng");
    // Scoped to the sidebar: the domain screen beside it browses by folder
    // too, and this is about the tree in the frame.
    const folder = await within(await sidebar()).findByRole("button", {
      name: "Expand notes",
    });
    // Nothing below the root was fetched: the tree is walked, not downloaded.
    expect(requested().some((path) => path.includes("path=notes"))).toBe(false);

    await userEvent.click(folder);

    expect(
      await within(await sidebar()).findByRole("link", { name: "Beta" }),
    ).toHaveAttribute("href", "/d/eng/e/notes/beta");
    expect(folder).toHaveAttribute("aria-expanded", "true");
    await waitFor(() => {
      expect(requested().some((path) => path.includes("path=notes"))).toBe(
        true,
      );
    });
  });

  it("marks the engram being read, and opens the folder holding it", async () => {
    serveInDomain();

    renderApp("/d/eng/e/notes/beta");

    const nav = await sidebar();
    // The folder on the way to it is open, because a mark nobody can see is
    // not a mark at all.
    expect(
      await within(nav).findByRole("button", { name: "Collapse notes" }),
    ).toHaveAttribute("aria-expanded", "true");
    const current = await within(nav).findByRole("link", { name: "Beta" });
    expect(current).toHaveAttribute("aria-current", "page");
    expect(
      within(nav).getByRole("link", { name: "Alpha" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("keeps a folder opened by hand open when the reader moves to a page off its path", async () => {
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const nav = await sidebar();
    const user = userEvent.setup();
    // Neither page is in `notes`: the reader opens it on purpose.
    await user.click(
      await within(nav).findByRole("button", { name: "Expand notes" }),
    );
    expect(
      await within(nav).findByRole("link", { name: "Beta" }),
    ).toBeVisible();

    await user.click(within(nav).getByRole("link", { name: "Old Way" }));
    await waitFor(() => {
      expect(
        within(nav).getByRole("link", { name: "Old Way" }),
      ).toHaveAttribute("aria-current", "page");
    });

    expect(
      within(nav).getByRole("button", { name: "Collapse notes" }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(within(nav).getByRole("link", { name: "Beta" })).toBeVisible();
  });

  it("opens the folder when the reader walks into it from the screen", async () => {
    serveInDomain();

    renderApp("/d/eng");
    const nav = await sidebar();
    const folder = await within(nav).findByRole("button", {
      name: "Expand notes",
    });
    expect(folder).toHaveAttribute("aria-expanded", "false");

    // Into the folder the long way round, through the screen's own browse
    // controls rather than the sidebar: the tree follows where the reader
    // went, not only where they arrived.
    const body = await screen.findByRole("main");
    const user = userEvent.setup();
    await user.click(
      await within(body).findByRole("button", { name: "notes" }),
    );
    await user.click(await within(body).findByRole("link", { name: /Beta/ }));

    await waitFor(() => {
      expect(folder).toHaveAttribute("aria-expanded", "true");
    });
    // The engram screen is its own chunk, and the navigation holds the old
    // screen until it arrives, so the row turns current with it.
    const beta = await within(nav).findByRole("link", { name: "Beta" });
    await waitFor(() => {
      expect(beta).toHaveAttribute("aria-current", "page");
    });
  });

  it("sets a retired engram apart in the tree rather than dropping it", async () => {
    serveInDomain();

    renderApp("/d/eng");

    const retired = await within(await sidebar()).findByRole("link", {
      name: "Old Way",
    });
    expect(retired).toBeVisible();
    // Slanted and in a lighter ink, not faded by opacity: the fade took the
    // row under 4.5:1.
    expect(retired.className).toContain("italic");
    expect(retired.className).not.toContain("opacity-60");
  });

  it("keeps the MANIFEST out of the tree, and pins no row for it", async () => {
    serveInDomain();

    renderApp("/d/eng");

    const nav = await sidebar();
    // The tree has arrived, so a row the engine listed would be on screen by
    // now if the sidebar were drawing it.
    await within(nav).findByRole("link", { name: "Alpha" });
    expect(within(nav).queryByRole("link", { name: "MANIFEST" })).toBeNull();
    expect(within(nav).queryByText("MANIFEST")).toBeNull();
  });

  it("marks the Overview row current on the domain page only", async () => {
    serveInDomain();

    renderApp("/d/eng?tags=eng");
    let nav = await sidebar();
    // A filter keeps the page it is on: the domain page, still current.
    expect(
      await within(nav).findByRole("link", { name: "Overview" }),
    ).toHaveAttribute("aria-current", "page");
    cleanup();

    renderApp("/d/eng?path=notes");
    nav = await sidebar();
    // A folder is its own page, and the folder's row is what is current.
    expect(
      await within(nav).findByRole("link", { name: "Overview" }),
    ).not.toHaveAttribute("aria-current");
    expect(
      await within(nav).findByRole("link", { name: "notes" }),
    ).toHaveAttribute("aria-current", "page");
    cleanup();

    renderApp("/d/eng/e/alpha");
    nav = await sidebar();
    expect(
      await within(nav).findByRole("link", { name: "Overview" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("says why a folder is empty instead of showing an empty one", async () => {
    serveInDomain({
      "/domains/eng/tree": () => {
        throw new ApiProblem(
          403,
          "forbidden",
          "this account may not browse eng",
        );
      },
    });

    renderApp("/d/eng");

    const alert = await within(await sidebar()).findByRole("alert");
    expect(alert).toHaveTextContent("this account may not browse eng");
    // Overview and the bar's switcher survive the failure: one domain refusing
    // to be browsed is not a reason to strand somebody in it.
    expect(
      within(await sidebar()).getByRole("link", { name: "Overview" }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "eng, switch domain" }),
    ).toBeVisible();
  });
});

/**
 * The sidebar is the widest thing on screen that is not what the reader came
 * for, so it folds down to a column of icons and stays that way: a choice
 * about the shape of the frame is a choice about every screen after it, not
 * about the one that happened to be open when it was made.
 */
describe("the sidebar's own width", () => {
  it("collapses to a rail and remembers it", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const collapse = await screen.findByRole("button", {
      name: "Collapse the sidebar",
    });
    expect(collapse).toHaveAttribute("aria-expanded", "true");
    // The same element the top bar's own disclosure names, so the two
    // controls of one sidebar say so in the same words.
    expect(collapse).toHaveAttribute("aria-controls", "domain-sidebar");

    await user.click(collapse);

    const nav = screen.getByRole("navigation", { name: "Domains" });
    // The listing is gone rather than squeezed into three rem of column.
    expect(within(nav).queryByRole("link", { name: /^eng/ })).toBeNull();
    const expand = within(nav).getByRole("button", {
      name: "Expand the sidebar",
    });
    expect(expand).toHaveAttribute("aria-expanded", "false");
    expect(expand).toHaveAttribute("aria-controls", "domain-sidebar");
    // What survives the fold is the way home, named for the keyboard and the
    // screen reader as much as for the eye.
    expect(
      within(nav).getByRole("button", { name: "All domains" }),
    ).toBeVisible();
    expect(localStorage.getItem("fluid.nav")).toBe("rail");
  });

  it("hands the keyboard on to the control that replaces the one it used", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    // By keyboard throughout: the control that folds the sidebar unmounts in
    // the act of doing it, and focus dropped on the document is a reader sent
    // back to the top of the page to find their way in again.
    (
      await screen.findByRole("button", { name: "Collapse the sidebar" })
    ).focus();
    await user.keyboard("{Enter}");

    const expand = await screen.findByRole("button", {
      name: "Expand the sidebar",
    });
    expect(document.activeElement).toBe(expand);

    await user.keyboard("{Enter}");
    expect(document.activeElement).toBe(
      await screen.findByRole("button", { name: "Collapse the sidebar" }),
    );
  });

  it("puts the fold in the sidebar's foot, after everything the sidebar lists", async () => {
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const nav = await sidebar();
    const launcher = await within(nav).findByRole("button", {
      name: "New engram",
    });
    const collapse = within(nav).getByRole("button", {
      name: "Collapse the sidebar",
    });
    // Below the tree and its launcher, in a foot of its own rather than
    // floating above the list.
    expect(
      launcher.compareDocumentPosition(collapse) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    const buttons = within(nav).getAllByRole("button");
    expect(buttons[buttons.length - 1]).toBe(collapse);
    // A wide window has no drawer, so no drawer head either.
    expect(
      within(nav).queryByRole("button", { name: "Close the sidebar" }),
    ).toBeNull();
  });

  it("reads a stored rail preference at mount", async () => {
    localStorage.setItem("fluid.nav", "rail");
    serveSignedIn();

    renderApp("/");

    expect(
      await screen.findByRole("button", { name: "Expand the sidebar" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Collapse the sidebar" }),
    ).toBeNull();
  });

  it("ignores a stored rail below the medium breakpoint", async () => {
    // The stored width is a choice about a column beside the reading surface,
    // and below `md` there is no such column: the sidebar is a drawer the top
    // bar opens over the screen. A rail applied there would hand a phone a
    // three-rem strip of icons, put the way out of it behind a control the
    // stylesheet hides, and leave the two disclosures of one sidebar saying
    // different things about whether it is open.
    localStorage.setItem("fluid.nav", "rail");
    stubNarrowWindow();
    serveSignedIn();

    renderApp("/");

    // The one control a narrow window offers says what the drawer is doing,
    // with nothing beside it to contradict it, and points at the drawer only
    // while there is one.
    const toggle = await screen.findByRole("button", { name: "Domains" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).not.toHaveAttribute("aria-controls");
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-controls", "domain-sidebar");
    const nav = await screen.findByRole("navigation", { name: "Domains" });
    expect(
      await within(nav).findByRole("link", { name: /^eng/ }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Expand the sidebar" }),
    ).toBeNull();
  });

  it("opens the phone drawer as a modal dialog headed by the domain", async () => {
    stubNarrowWindow();
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    const toggle = await screen.findByRole("button", { name: "Domains" });
    // Shut, there is no drawer to reach.
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const drawer = await screen.findByRole("dialog", { name: "eng" });
    expect(drawer).toHaveAttribute("aria-modal", "true");
    // The drawer takes the keyboard, and holds the tree for the current mode
    // with no fold control: there is no rail on a phone.
    expect(document.activeElement).toBe(drawer);
    const nav = within(drawer).getByRole("navigation", { name: "Domain eng" });
    expect(
      await within(nav).findByRole("link", { name: "Overview" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Collapse the sidebar" }),
    ).toBeNull();

    // Tab never leaves it: from the last control it wraps to the first.
    const close = within(drawer).getByRole("button", {
      name: "Close the sidebar",
    });
    close.focus();
    await user.tab({ shift: true });
    expect(drawer.contains(document.activeElement)).toBe(true);
    for (let i = 0; i < 12; i += 1) {
      await user.tab();
      expect(drawer.contains(document.activeElement)).toBe(true);
    }

    await user.click(close);
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    // The keyboard goes back to the control that opened it.
    await waitFor(() => {
      expect(document.activeElement).toBe(toggle);
    });
  });

  it("shuts the drawer on Escape from anywhere in it, and on a followed link", async () => {
    stubNarrowWindow();
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    const toggle = await screen.findByRole("button", { name: "Domains" });
    await user.click(toggle);
    await screen.findByRole("dialog", { name: "eng" });
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await waitFor(() => {
      expect(document.activeElement).toBe(toggle);
    });

    await user.click(toggle);
    const drawer = await screen.findByRole("dialog", { name: "eng" });
    await user.click(
      await within(drawer).findByRole("link", { name: "Overview" }),
    );
    await screen.findByRole("heading", { level: 1, name: "eng" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
  });

  it("keeps the drawer open when Escape shuts a dialog opened from it, and returns to the launcher", async () => {
    stubNarrowWindow();
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    const toggle = await screen.findByRole("button", { name: "Domains" });
    await user.click(toggle);
    const drawer = await screen.findByRole("dialog", { name: "eng" });
    const launcher = await within(drawer).findByRole("button", {
      name: "New engram",
    });
    await user.click(launcher);
    await screen.findByRole("dialog", { name: /^New engram in/ });
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(
        screen.queryByRole("dialog", { name: /^New engram in/ }),
      ).toBeNull();
    });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("dialog", { name: "eng" })).toBeInTheDocument();
    await waitFor(() => {
      expect(document.activeElement).toBe(launcher);
    });
  });

  it("puts a long page's outline at the top of the phone drawer", async () => {
    stubNarrowWindow();
    serveInDomain({
      "/domains/eng/engrams/alpha": () => ({
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        url: "crystalline://eng/alpha",
        content: "# Alpha\n\n## One\n\nx\n\n## Two\n\nx\n\n## Three\n\nx\n",
        frontmatter: {},
        observations: [],
        relations: [],
        links: [],
      }),
    });
    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Domains" }));
    const drawer = await screen.findByRole("dialog", { name: "eng" });
    const nav = await within(drawer).findByRole("navigation", {
      name: "On this page",
    });
    expect(
      Array.from(nav.querySelectorAll("a")).map((a) => a.textContent),
    ).toEqual(["One", "Two", "Three"]);
  });

  it("shuts the drawer for good when the window grows past the breakpoint", async () => {
    let wide = false;
    const listeners = new Set<(event: { matches: boolean }) => void>();
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: (query: string) => ({
        get matches() {
          return query.includes("min-width") ? wide : false;
        },
        media: query,
        addEventListener: (
          _: string,
          fn: (event: { matches: boolean }) => void,
        ) => {
          if (query.includes("min-width")) {
            listeners.add(fn);
          }
        },
        removeEventListener: (
          _: string,
          fn: (event: { matches: boolean }) => void,
        ) => {
          listeners.delete(fn);
        },
      }),
    });
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    const toggle = await screen.findByRole("button", { name: "Domains" });
    await user.click(toggle);
    await screen.findByRole("dialog", { name: "eng" });

    act(() => {
      wide = true;
      listeners.forEach((fn) => {
        fn({ matches: wide });
      });
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => {
      wide = false;
      listeners.forEach((fn) => {
        fn({ matches: wide });
      });
    });
    // Narrow again, and the drawer stays shut until it is asked for.
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(toggle).toHaveAttribute("aria-expanded", "false");
  });

  it("keeps folders opened by hand across the drawer and the column", async () => {
    const resize = stubResizableWindow();
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    const toggle = await screen.findByRole("button", { name: "Domains" });
    await user.click(toggle);
    let drawer = await screen.findByRole("dialog", { name: "eng" });
    await user.click(
      await within(drawer).findByRole("button", { name: "Expand notes" }),
    );
    // Shut and opened again: the folder is still open.
    await user.click(
      within(drawer).getByRole("button", { name: "Close the sidebar" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    await user.click(toggle);
    drawer = await screen.findByRole("dialog", { name: "eng" });
    expect(
      await within(drawer).findByRole("button", { name: "Collapse notes" }),
    ).toHaveAttribute("aria-expanded", "true");

    // And the column past the breakpoint draws it open as well.
    resize(true);
    const nav = await sidebar();
    expect(
      await within(nav).findByRole("button", { name: "Collapse notes" }),
    ).toBeVisible();
    // Shut there, it is shut in the drawer too.
    await user.click(
      within(nav).getByRole("button", { name: "Collapse notes" }),
    );
    resize(false);
    await user.click(toggle);
    drawer = await screen.findByRole("dialog", { name: "eng" });
    expect(
      await within(drawer).findByRole("button", { name: "Expand notes" }),
    ).toHaveAttribute("aria-expanded", "false");
  });

  it("keeps a dialog opened from the drawer, and its text, when the window grows", async () => {
    const resize = stubResizableWindow();
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Domains" }));
    const drawer = await screen.findByRole("dialog", { name: "eng" });
    await user.click(
      await within(drawer).findByRole("button", { name: "New engram" }),
    );
    const create = await screen.findByRole("dialog", {
      name: /^New engram in/,
    });
    const title = within(create).getByRole("textbox", { name: /title/i });
    await user.type(title, "Half typed");

    resize(true);
    // The drawer is gone, the dialog and what was typed in it are not.
    expect(screen.queryByRole("dialog", { name: "eng" })).toBeNull();
    const still = screen.getByRole("dialog", { name: /^New engram in/ });
    expect(within(still).getByRole("textbox", { name: /title/i })).toHaveValue(
      "Half typed",
    );
  });

  it("names the drawer All domains outside a domain", async () => {
    stubNarrowWindow();
    serveSignedIn();

    renderApp("/");
    await userEvent.click(
      await screen.findByRole("button", { name: "Domains" }),
    );
    const drawer = await screen.findByRole("dialog", { name: "All domains" });
    expect(
      within(drawer).getByRole("navigation", { name: "Domains" }),
    ).toBeVisible();
    expect(
      within(drawer).getByRole("button", { name: "Close the sidebar" }),
    ).toBeVisible();
  });

  it("gives the keyboard back to New engram when its dialog is dismissed", async () => {
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    const nav = await sidebar();
    const launcher = await within(nav).findByRole("button", {
      name: "New engram",
    });
    await user.click(launcher);
    await screen.findByRole("dialog", { name: /^New engram in/ });
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(launcher);
    });

    // The same from the rail's own launcher.
    await user.click(
      within(nav).getByRole("button", { name: "Collapse the sidebar" }),
    );
    const rail = within(nav).getByRole("button", { name: "New engram" });
    await user.click(rail);
    await screen.findByRole("dialog", { name: /^New engram in/ });
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => {
      expect(document.activeElement).toBe(rail);
    });
  });

  it("offers New engram in the sidebar on an engram page but not on the front page", async () => {
    serveInDomain();

    renderApp("/d/eng");
    let nav = await sidebar();
    await within(nav).findByRole("link", { name: "Alpha" });
    // The front page's engram list carries its own launcher.
    expect(
      within(nav).queryByRole("button", { name: "New engram" }),
    ).toBeNull();
    cleanup();

    renderApp("/d/eng/e/alpha");
    nav = await sidebar();
    expect(
      await within(nav).findByRole("button", { name: "New engram" }),
    ).toBeVisible();
    // A document glyph on every engram row, drawn and not spoken.
    const alpha = within(nav).getByRole("link", { name: "Alpha" });
    expect(alpha.querySelector("svg")?.getAttribute("class")).toContain(
      "lucide-file-text",
    );
    expect(alpha).toHaveAttribute("aria-current", "page");
    expect(
      within(nav).getByRole("link", { name: "Overview" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("survives a move to another screen", async () => {
    localStorage.setItem("fluid.nav", "rail");
    serveInDomain();

    renderApp("/");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "All domains" }),
    );
    await screen.findByRole("heading", { name: "Home" });

    // Still folded on the screen it landed on: the choice is the frame's, not
    // one screen's.
    expect(
      await screen.findByRole("button", { name: "Expand the sidebar" }),
    ).toBeVisible();
  });

  it("keeps a launcher inside the rail while a domain is open", async () => {
    localStorage.setItem("fluid.nav", "rail");
    serveInDomain();

    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    const nav = await sidebar();
    // The folded sidebar, not the one that still has room for a tree.
    expect(
      within(nav).getByRole("button", { name: "Expand the sidebar" }),
    ).toBeVisible();
    expect(within(nav).queryByRole("link", { name: "Alpha" })).toBeNull();

    await user.click(within(nav).getByRole("button", { name: "New engram" }));

    expect(
      await screen.findByRole("dialog", { name: /new engram/i }),
    ).toBeVisible();
  });

  it("leaves the launcher to the domain's own home screen", async () => {
    localStorage.setItem("fluid.nav", "rail");
    serveInDomain();

    renderApp("/d/eng");
    const nav = await sidebar();
    await screen.findByRole("heading", { level: 1, name: "eng" });

    // The frame folds to a rail and still keeps out of the way: this screen
    // carries its own launcher, prefilled with the folder being browsed, and
    // a second one in the rail beside it would be two controls for one act.
    // The rule is `DomainNav`'s, unchanged by the fold.
    expect(
      within(nav).queryByRole("button", { name: "New engram" }),
    ).toBeNull();
    expect(
      within(await screen.findByRole("main")).getByRole("button", {
        name: "New engram",
      }),
    ).toBeVisible();
  });

  it("unfolds again, and remembers that too", async () => {
    localStorage.setItem("fluid.nav", "rail");
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Expand the sidebar" }),
    );

    const nav = screen.getByRole("navigation", { name: "Domains" });
    expect(
      await within(nav).findByRole("link", { name: /^eng/ }),
    ).toBeVisible();
    expect(localStorage.getItem("fluid.nav")).toBe("expanded");
  });
});

/**
 * The other half of the frame's shape: how wide what you came to read is
 * allowed to be. The sidebar folding is a choice about the column beside the
 * content; this is a choice about the content itself, and it is remembered the
 * same way, for the same reason.
 */
describe("the content's own width", () => {
  it("takes the whole window and remembers it", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    expect(await screen.findByRole("main")).not.toHaveAttribute("data-width");
    // In the frame's own avatar menu rather than on the screen inside it: this
    // is a choice about every screen, and it is drawn with the theme.
    const menu = await openAvatar(user);
    const group = within(menu).getByRole("group", { name: "Width" });
    expect(
      within(group).getByRole("menuitemradio", { name: "Reading" }),
    ).toBeChecked();

    await user.click(
      within(group).getByRole("menuitemradio", { name: "Full" }),
    );

    // One attribute on the frame is the whole of what the stylesheet reads:
    // the measure is lifted under it, everywhere, with no screen saying so.
    expect(screen.getByRole("main", { hidden: true })).toHaveAttribute(
      "data-width",
      "full",
    );
    expect(
      within(group).getByRole("menuitemradio", { name: "Full" }),
    ).toBeChecked();
    expect(localStorage.getItem("fluid.layout.width")).toBe("full");
    // A choice in the Display group does not close the menu.
    expect(screen.getByRole("menu")).toBeVisible();
  });

  it("reads a stored full width at mount", async () => {
    localStorage.setItem("fluid.layout.width", "full");
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const menu = await openAvatar(user);

    expect(
      within(menu).getByRole("menuitemradio", { name: "Full" }),
    ).toBeChecked();
    expect(screen.getByRole("main", { hidden: true })).toHaveAttribute(
      "data-width",
      "full",
    );
  });

  it("lifts the frame's own cap at full width, header and body alike", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const main = await screen.findByRole("main");
    expect(main.parentElement?.className).toContain("max-w-350");
    const headerRow = document.querySelector("header > div");
    expect(headerRow?.className).toContain("max-w-350");

    await openAvatar(user);
    await user.click(screen.getByRole("menuitemradio", { name: "Full" }));

    // Full width means the window: dropping the details column and lifting
    // the measure while the frame keeps its cap would hand back the room and
    // then refuse to use it.
    expect(main.parentElement?.className).not.toContain("max-w-350");
    expect(headerRow?.className).not.toContain("max-w-350");

    await user.click(screen.getByRole("menuitemradio", { name: "Reading" }));
    expect(main.parentElement?.className).toContain("max-w-350");
    expect(headerRow?.className).toContain("max-w-350");
  });

  it("goes back to the measure, and remembers that too", async () => {
    localStorage.setItem("fluid.layout.width", "full");
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    await openAvatar(user);
    await user.click(screen.getByRole("menuitemradio", { name: "Reading" }));

    expect(screen.getByRole("main", { hidden: true })).not.toHaveAttribute(
      "data-width",
    );
    expect(localStorage.getItem("fluid.layout.width")).toBe("reading");
  });

  it("answers the bare backslash from anywhere", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    await screen.findByRole("main");
    await user.keyboard("\\");

    expect(screen.getByRole("main")).toHaveAttribute("data-width", "full");

    await user.keyboard("\\");

    expect(screen.getByRole("main")).not.toHaveAttribute("data-width");
  });

  it("answers it on a keyboard where the backslash needs a modifier", async () => {
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("main");

    // Synthesized rather than typed: jsdom's keyboard has one layout, and
    // these are the events other layouts send. On a German, French, Spanish,
    // Italian or Nordic keyboard the backslash IS a modified key - AltGr on
    // Windows and Linux, which a browser reports as Ctrl and Alt together,
    // and Shift+Option on macOS. The key is "\\" in every one of them, so
    // the shortcut the help overlay advertises has to fire.
    fireEvent.keyDown(document.body, {
      key: "\\",
      ctrlKey: true,
      altKey: true,
    });

    expect(screen.getByRole("main")).toHaveAttribute("data-width", "full");

    fireEvent.keyDown(document.body, {
      key: "\\",
      shiftKey: true,
      altKey: true,
    });

    expect(screen.getByRole("main")).not.toHaveAttribute("data-width");
  });

  it("leaves Cmd and Ctrl on the backslash to whoever owns them", async () => {
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("main");

    // Without Alt beside it, a modified backslash is the browser's or the
    // system's rather than this app's.
    fireEvent.keyDown(document.body, { key: "\\", metaKey: true });
    fireEvent.keyDown(document.body, { key: "\\", ctrlKey: true });

    expect(screen.getByRole("main")).not.toHaveAttribute("data-width");
  });

  it("leaves the backslash alone while somebody is writing one", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const search = await screen.findByRole("searchbox", { name: "Search" });
    await user.click(search);
    await user.keyboard("\\");

    // The character lands in the field it was typed into and nothing else
    // happens: a bare key is only a shortcut where no field has the focus.
    expect(search).toHaveValue("\\");
    expect(screen.getByRole("main")).not.toHaveAttribute("data-width");
  });

  it("is on the palette and in the shortcut map", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    await screen.findByRole("main");
    await user.keyboard("{Meta>}k{/Meta}");

    await user.click(
      await screen.findByRole("option", { name: /toggle full width/i }),
    );

    expect(screen.getByRole("main")).toHaveAttribute("data-width", "full");

    // And written down where a reader goes looking for the key itself.
    await user.keyboard("?");
    const help = await screen.findByRole("dialog", {
      name: /keyboard shortcuts/i,
    });
    expect(help).toHaveTextContent(/full width/i);
  });
});

describe("the text size", () => {
  it("switches the document to large text and remembers it", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const main = await screen.findByRole("main");
    expect(main).not.toHaveAttribute("data-text");

    await openAvatar(user);
    await user.click(screen.getByRole("menuitemradio", { name: "Large" }));

    // One attribute on the frame is the whole of what the stylesheet reads,
    // for the rendered document and the editor alike.
    expect(main).toHaveAttribute("data-text", "large");
    expect(screen.getByRole("menuitemradio", { name: "Large" })).toBeChecked();
    expect(localStorage.getItem("fluid.layout.text")).toBe("large");
    expect(screen.getByRole("menu")).toBeVisible();
  });

  it("reads a stored large text at mount, and goes back", async () => {
    localStorage.setItem("fluid.layout.text", "large");
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    expect(await screen.findByRole("main")).toHaveAttribute(
      "data-text",
      "large",
    );
    await openAvatar(user);
    await user.click(screen.getByRole("menuitemradio", { name: "Regular" }));
    expect(screen.getByRole("main", { hidden: true })).not.toHaveAttribute(
      "data-text",
    );
    expect(localStorage.getItem("fluid.layout.text")).toBe("regular");
  });

  it("offers the toggle on the palette", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    await screen.findByRole("main");
    await user.keyboard("{Meta>}k{/Meta}");
    await user.type(screen.getByRole("combobox"), "large text");
    await user.click(
      await screen.findByRole("option", { name: "Toggle large text" }),
    );
    expect(screen.getByRole("main")).toHaveAttribute("data-text", "large");
  });
});

describe("the Handbook and the shortcut map in the avatar menu", () => {
  it("leads to the Handbook in a new tab and to the shortcut map", async () => {
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("main");
    const user = userEvent.setup();

    await openAvatar(user);
    const handbook = await screen.findByRole("menuitem", {
      name: "Handbook (opens in a new tab)",
    });
    expect(handbook).toHaveAttribute(
      "href",
      "https://jordiboehme.github.io/crystalline/",
    );
    expect(handbook).toHaveAttribute("target", "_blank");
    expect(handbook.getAttribute("rel") ?? "").toMatch(/noreferrer/);

    await user.click(
      screen.getByRole("menuitem", { name: /^Keyboard shortcuts/ }),
    );
    const help = await screen.findByRole("dialog", {
      name: /keyboard shortcuts/i,
    });
    const more = within(help).getByRole("link", { name: /Handbook/ });
    expect(more).toHaveAttribute(
      "href",
      "https://jordiboehme.github.io/crystalline/",
    );
    expect(more).toHaveAttribute("target", "_blank");
  });

  it("opens the map from the keyboard and gives focus back to the avatar", async () => {
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("main");
    const user = userEvent.setup();

    const avatar = await screen.findByRole("button", { name: "Ada Lovelace" });
    avatar.focus();
    await user.keyboard("{Enter}");
    await screen.findByRole("menuitem", { name: "Maintenance" });
    await user.click(
      screen.getByRole("menuitem", { name: /^Keyboard shortcuts/ }),
    );
    expect(
      await screen.findByRole("dialog", { name: /keyboard shortcuts/i }),
    ).toBeVisible();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(avatar).toHaveFocus();
    });
  });

  it("still opens the map with the question mark", async () => {
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("main");
    const user = userEvent.setup();
    await user.keyboard("?");

    expect(
      await screen.findByRole("dialog", { name: /keyboard shortcuts/i }),
    ).toBeVisible();
  });

  it("opens the Handbook from the palette", async () => {
    serveSignedIn();
    const open = vi.spyOn(window, "open").mockReturnValue(null);

    renderApp("/");
    await screen.findByRole("main");
    const user = userEvent.setup();
    await user.keyboard("{Meta>}k{/Meta}");
    await user.click(
      await screen.findByRole("option", { name: /open the handbook/i }),
    );

    expect(open).toHaveBeenCalledWith(
      "https://jordiboehme.github.io/crystalline/",
      "_blank",
      "noopener,noreferrer",
    );
    open.mockRestore();
  });
});

/**
 * The avatar menu (spec 4.3): one menu for who you are, Maintenance and the
 * Display choices, as the avatar in the bar opens it.
 */
describe("the avatar menu", () => {
  it("shows the initial, and names the person", async () => {
    serveSignedIn();

    renderApp("/");
    const avatar = await screen.findByRole("button", { name: "Ada Lovelace" });

    expect(avatar).toHaveTextContent("A");
  });

  it("changes the theme and keeps the menu open", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const menu = await openAvatar(user);
    const theme = within(menu).getByRole("group", { name: "Theme" });

    await user.click(
      within(theme).getByRole("menuitemradio", { name: "Dark" }),
    );

    await waitFor(() => {
      expect(document.documentElement.dataset.theme).toBe("dark");
    });
    expect(screen.getByRole("menu")).toBeVisible();
    expect(
      within(theme).getByRole("menuitemradio", { name: "Dark" }),
    ).toBeChecked();
    // And a second choice in the same visit, without opening it again.
    await user.click(
      within(theme).getByRole("menuitemradio", { name: "Light" }),
    );
    await waitFor(() => {
      expect(document.documentElement.dataset.theme).toBe("light");
    });
    expect(screen.getByRole("menu")).toBeVisible();
  });

  it("moves within a radio group with the left and right arrows", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const menu = await openAvatar(user);
    const theme = within(menu).getByRole("group", { name: "Theme" });
    const system = within(theme).getByRole("menuitemradio", { name: "System" });
    const light = within(theme).getByRole("menuitemradio", { name: "Light" });
    const dark = within(theme).getByRole("menuitemradio", { name: "Dark" });

    system.focus();
    await user.keyboard("{ArrowRight}");
    expect(light).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(dark).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(system).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(dark).toHaveFocus();
    // Space or Enter chooses it, and the menu stays.
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(document.documentElement.dataset.theme).toBe("dark");
    });
    expect(screen.getByRole("menu")).toBeVisible();
  });

  it("closes on Escape and returns focus to the avatar", async () => {
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const avatar = await screen.findByRole("button", { name: "Ada Lovelace" });
    await openAvatar(user);

    await user.keyboard("{Escape}");

    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(avatar).toHaveFocus();
  });

  it("keeps the shortcut map on a phone and leaves the width choice to the desktop", async () => {
    stubNarrowWindow();
    serveSignedIn();

    renderApp("/");
    const user = userEvent.setup();
    const menu = await openAvatar(user);

    expect(
      within(menu).getByRole("menuitem", { name: /^Keyboard shortcuts/ }),
    ).toBeVisible();
    expect(within(menu).queryByRole("group", { name: "Width" })).toBeNull();
    expect(within(menu).getByRole("group", { name: "Theme" })).toBeVisible();
  });

  it("marks Maintenance as the current screen while you are on it", async () => {
    serveSignedIn();

    renderApp("/maintenance");
    const user = userEvent.setup();
    const menu = await openAvatar(user);

    const item = within(menu).getByRole("menuitem", { name: "Maintenance" });
    expect(item).toHaveAttribute("aria-current", "page");
    expect(item.className).toContain("font-medium");
    // The shared menu row (components/menu.ts), the same face as every menu.
    expect(item.className).toContain("menu-row");
    expect(
      within(menu).getByRole("menuitem", { name: "Profile" }).className,
    ).not.toContain("font-medium");
    expect(
      within(menu).getByRole("menuitem", { name: "Profile" }).className,
    ).toContain("menu-row");
    expect(
      within(menu).getByRole("menuitem", { name: "Profile" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("puts the keyboard on the first row when the menu is opened with Enter", async () => {
    serveSignedIn();

    renderApp("/");
    const avatar = await screen.findByRole("button", { name: "Ada Lovelace" });
    avatar.focus();
    const user = userEvent.setup();
    await user.keyboard("{Enter}");
    await screen.findByRole("menuitem", { name: "Maintenance" });

    await waitFor(() => {
      expect(screen.getByRole("menuitem", { name: "Profile" })).toHaveFocus();
    });
  });

  it("is a neutral circle with a way to log in for an anonymous reader", async () => {
    serve({
      "/auth/me": () => meResponse({ anonymous: true }),
      "/domains": domainsResponse,
    });

    renderApp("/");
    const user = userEvent.setup();
    const menu = await openAvatar(user, "Viewing anonymously");

    expect(within(menu).getByText("No account on this session")).toBeVisible();
    expect(
      within(menu).getByRole("menuitem", { name: "Log in" }),
    ).toHaveAttribute("href", "/login");
    // What works without an account stays.
    expect(
      within(menu).getByRole("menuitem", { name: "Maintenance" }),
    ).toBeVisible();
    expect(within(menu).getByRole("group", { name: "Theme" })).toBeVisible();
    expect(
      within(menu).getByRole("group", { name: "Text size" }),
    ).toBeVisible();
    expect(within(menu).getByRole("group", { name: "Width" })).toBeVisible();
    expect(
      within(menu).getByRole("menuitem", { name: /^Keyboard shortcuts/ }),
    ).toBeVisible();
    expect(
      within(menu).getByRole("menuitem", { name: /^Handbook/ }),
    ).toBeVisible();
    // And nothing that needs an account.
    for (const gone of ["Profile", "Users", "Settings", "Log out"]) {
      expect(within(menu).queryByRole("menuitem", { name: gone })).toBeNull();
    }
  });

  it("offers the marks setting in the Display group, off until chosen", async () => {
    localStorage.clear();
    serve({
      "/auth/me": () => meResponse({ anonymous: true }),
      "/domains": domainsResponse,
    });

    renderApp("/");
    const user = userEvent.setup();
    const menu = await openAvatar(user, "Viewing anonymously");

    const item = within(menu).getByRole("menuitemcheckbox", {
      name: "Show Markdown marks on the line I edit",
    });
    expect(item).toHaveAttribute("aria-checked", "false");
    await user.click(item);
    expect(item).toHaveAttribute("aria-checked", "true");
    expect(localStorage.getItem("fluid-marks-on-edit-line")).toBe("on");
    await user.click(item);
    expect(item).toHaveAttribute("aria-checked", "false");
    expect(localStorage.getItem("fluid-marks-on-edit-line")).toBeNull();
  });
});

/**
 * The domain switcher in the top bar (spec 4.1): the current domain by name,
 * "All domains" outside one, and a menu with a filter field, the way home,
 * every domain with its chip and count, a check on the current one, and
 * "New domain" for an admin.
 */
describe("the domain switcher in the top bar", () => {
  async function openSwitcher(name: string): Promise<HTMLElement> {
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name }));
    return screen.findByRole("menu");
  }

  it("reads All domains outside a domain and leads home with the count", async () => {
    serveSignedIn({ "/reflection/domains": () => ({ domains: [] }) });
    renderApp("/");
    const menu = await openSwitcher("All domains, switch domain");
    const home = await within(menu).findByRole("menuitem", {
      name: /^All domains/,
    });
    expect(home).toHaveAttribute("href", "/");
    expect(home).toHaveTextContent("1");
  });

  it("checks the current domain and no other", async () => {
    serveInDomain({ "/reflection/domains": () => ({ domains: [] }) });
    renderApp("/d/eng");
    const menu = await openSwitcher("eng, switch domain");
    const eng = await within(menu).findByRole("menuitem", { name: /^eng/ });
    expect(eng).toHaveAttribute("aria-current", "true");
    // Said in words too, for a reader that does not announce aria-current.
    expect(eng).toHaveTextContent("(current)");
    expect(eng).toHaveAttribute("href", "/d/eng");
    expect(eng.querySelector("svg")).not.toBeNull();
    const ops = within(menu).getByRole("menuitem", { name: /^ops/ });
    expect(ops).not.toHaveAttribute("aria-current");
    expect(ops.querySelector("svg")).toBeNull();
  });

  it("narrows the list as the filter is typed, and says when nothing matches", async () => {
    serveInDomain({ "/reflection/domains": () => ({ domains: [] }) });
    renderApp("/d/eng");
    const menu = await openSwitcher("eng, switch domain");
    const field = await within(menu).findByRole("searchbox", {
      name: "Find a domain",
    });
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    const user = userEvent.setup();
    await user.type(field, "OP");
    expect(field).toHaveValue("OP");
    expect(within(menu).getByRole("menuitem", { name: /^ops/ })).toBeVisible();
    expect(within(menu).queryByRole("menuitem", { name: /^eng/ })).toBeNull();
    await user.type(field, "zz");
    expect(within(menu).getByText("No domain matches.")).toBeVisible();
    // The way home is never filtered away.
    expect(
      within(menu).getByRole("menuitem", { name: /^All domains/ }),
    ).toBeVisible();
  });

  it("works from the keyboard: open, type, down into the list, Escape back", async () => {
    serveInDomain({ "/reflection/domains": () => ({ domains: [] }) });
    renderApp("/d/eng");
    const trigger = await screen.findByRole("button", {
      name: "eng, switch domain",
    });
    trigger.focus();
    const user = userEvent.setup();
    await user.keyboard("{Enter}");
    const field = await screen.findByRole("searchbox", {
      name: "Find a domain",
    });
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    // A letter goes into the field, not to the menu's typeahead.
    await user.keyboard("o");
    expect(field).toHaveValue("o");
    expect(field).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(
      screen.getByRole("menuitem", { name: /^All domains/ }),
    ).toHaveFocus();
    // Up from the first row is back in the field.
    await user.keyboard("{ArrowUp}");
    expect(field).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: /^ops/ })).toHaveFocus();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(trigger).toHaveFocus();
  });

  it("opens the one domain left by the filter on Enter", async () => {
    serveInDomain({
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/ops/tree": () => ({
        domain: "ops",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/domains/ops/manifest": () => ({ domain: "ops", markdown: "" }),
      "/domains/ops/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
    });
    renderApp("/d/eng");
    const menu = await openSwitcher("eng, switch domain");
    const field = await within(menu).findByRole("searchbox", {
      name: "Find a domain",
    });
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    const user = userEvent.setup();
    // Two matches: Enter does nothing yet.
    await user.keyboard("{Enter}");
    expect(screen.getByRole("menu")).toBeVisible();
    await user.keyboard("op{Enter}");
    expect(
      await screen.findByRole("heading", { level: 1, name: "ops" }),
    ).toBeVisible();
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
  });

  it("offers New domain to an admin only", async () => {
    serveInDomain({ "/reflection/domains": () => ({ domains: [] }) });
    renderApp("/d/eng");
    const menu = await openSwitcher("eng, switch domain");
    await within(menu).findByRole("menuitem", { name: /^eng/ });
    expect(
      within(menu).queryByRole("menuitem", { name: "New domain" }),
    ).toBeNull();
  });

  it("opens the register dialog from New domain for an admin", async () => {
    serveInDomain({
      "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
      "/reflection/domains": () => ({ domains: [] }),
      "/settings/github": () => ({ enabled: false, connected: false }),
    });
    renderApp("/d/eng");
    const menu = await openSwitcher("eng, switch domain");
    await userEvent.click(
      await within(menu).findByRole("menuitem", { name: "New domain" }),
    );
    expect(await screen.findByRole("dialog")).toBeVisible();
  });

  it("lists a Reflection-only domain and marks Reflection, view only and private rows", async () => {
    serveInDomain({
      "/domains": () => ({
        behavior: [],
        domains: [
          { name: "eng", kind: "file", engrams: 4, when_to_use: [] },
          {
            name: "ops",
            kind: "file",
            engrams: 2,
            read_only: true,
            when_to_use: [],
          },
          {
            name: "lab",
            kind: "file",
            engrams: 1,
            private: true,
            when_to_use: [],
          },
        ],
      }),
      "/reflection/domains": () => ({
        domains: [
          {
            domain: "eng",
            state: "on",
            right: "curate",
            engrams_visible: true,
            home: "here",
            home_url: null,
            open_threads: 0,
          },
          {
            domain: "guild",
            state: "on",
            right: "edit",
            engrams_visible: false,
            home: "here",
            home_url: null,
            open_threads: 0,
          },
        ],
      }),
    });
    renderApp("/d/eng");
    const menu = await openSwitcher("eng, switch domain");
    const guild = await within(menu).findByRole("menuitem", {
      name: /^guild/,
    });
    // A domain reached only through its Reflection leads into it.
    expect(guild).toHaveAttribute("href", "/d/guild/reflection");
    expect(guild).toHaveTextContent("Reflection");
    expect(
      within(menu).getByRole("menuitem", { name: /^eng/ }),
    ).toHaveTextContent("Reflection");
    expect(
      within(menu).getByRole("menuitem", { name: /^ops/ }),
    ).toHaveTextContent("view only");
    expect(
      within(menu).getByRole("menuitem", { name: /^lab/ }),
    ).toHaveTextContent("private");
    // Four domains in all, the Reflection-only one included.
    expect(
      within(menu).getByRole("menuitem", { name: /^All domains/ }),
    ).toHaveTextContent("4");
  });
});

/**
 * On a phone the bar has no room for the mode switch or a search field: the
 * switch gets a row of its own under the bar, and search is an icon that
 * opens the palette.
 */
describe("the top bar on a phone", () => {
  it("puts the mode switch under the bar, and the search icon opens the palette", async () => {
    stubNarrowWindow();
    serveInDomain({
      "/reflection/domains": () => ({ domains: [] }),
      "/domains/eng/reflection": () => ({
        domain: "eng",
        enabled: true,
        state: "on",
        home: "local",
        home_url: null,
        right: "curate",
        engrams_visible: true,
        pending_engrams: [],
      }),
    });
    renderApp("/d/eng");
    const modes = await screen.findByRole("navigation", {
      name: "Domain view",
    });
    const bar =
      screen.getByRole("button", { name: "Domains" }).closest("header") ??
      undefined;
    expect(defined(bar, "the top bar").contains(modes)).toBe(false);
    const icon = screen.getByRole("button", { name: "Open search" });
    expect(icon).toHaveAttribute("aria-haspopup", "dialog");
    await userEvent.click(icon);
    expect(await screen.findByRole("dialog")).toBeVisible();
    // Closing hands the focus back to the icon that opened it.
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(icon).toHaveFocus();
  });
});
