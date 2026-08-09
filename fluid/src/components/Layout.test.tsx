/**
 * The frame's own behavior: the domain list it fetches, where the search box
 * sends you, what the theme control writes, what logging out does, and what
 * the sidebar becomes once a domain is open.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api, setCsrfToken } from "../api/client";
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
    engrams: [
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
    "/domains/eng/engrams": () => ({
      mode: "text",
      total: 0,
      page: 1,
      limit: 50,
      count: 0,
      hits: [],
    }),
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

beforeEach(() => {
  apiMock.mockReset();
  setCsrfTokenMock.mockReset();
  document.documentElement.removeAttribute("data-theme");
});

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
    // A refusal is rendered where it happened, never as a bounce to the login
    // screen the caller is already past.
    expect(screen.queryByLabelText("Password")).not.toBeInTheDocument();
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

  it("writes the chosen theme onto the document", async () => {
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /^Theme:/ }));
    await user.click(
      await screen.findByRole("menuitemradio", { name: "Dark" }),
    );

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
    // The switcher says where you are, and is the control that moves you.
    expect(
      await within(nav).findByRole("button", { name: "Domain: eng" }),
    ).toBeVisible();
    // The way back to everything stays on screen rather than being a browser
    // button somebody has to remember.
    expect(
      within(nav).getByRole("link", { name: "All domains" }),
    ).toHaveAttribute("href", "/");
    // And below it, what this domain holds: its folders and its engrams.
    expect(
      await within(nav).findByRole("button", { name: "notes" }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(within(nav).getByRole("link", { name: "Alpha" })).toHaveAttribute(
      "href",
      "/d/eng/e/alpha",
    );
    // The flat list is gone: two lists of domains at once would be two answers
    // to the same question.
    expect(within(nav).queryByRole("link", { name: /^ops/ })).toBeNull();
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
      await screen.findByRole("button", { name: "Domain: eng" }),
    );

    // Every domain is offered with what it holds, which is what makes the
    // choice between them a choice rather than a guess.
    const other = await screen.findByRole("menuitemradio", { name: /^ops/ });
    expect(other).toHaveTextContent("2");
    await user.click(other);

    expect(
      await screen.findByRole("heading", { level: 1, name: "ops" }),
    ).toBeVisible();
    expect(
      await screen.findByRole("button", { name: "Domain: ops" }),
    ).toBeVisible();
  });

  it("asks for a folder only when it is opened", async () => {
    serveInDomain();

    renderApp("/d/eng");
    // Scoped to the sidebar: the domain screen beside it browses by folder
    // too, and this is about the tree in the frame.
    const folder = await within(await sidebar()).findByRole("button", {
      name: "notes",
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
      await within(nav).findByRole("button", { name: "notes" }),
    ).toHaveAttribute("aria-expanded", "true");
    const current = await within(nav).findByRole("link", { name: "Beta" });
    expect(current).toHaveAttribute("aria-current", "page");
    expect(
      within(nav).getByRole("link", { name: "Alpha" }),
    ).not.toHaveAttribute("aria-current");
  });

  it("opens the folder when the reader walks into it from the screen", async () => {
    serveInDomain();

    renderApp("/d/eng");
    const nav = await sidebar();
    const folder = await within(nav).findByRole("button", { name: "notes" });
    expect(folder).toHaveAttribute("aria-expanded", "false");

    // Into the folder the long way round, through the screen's own browse
    // controls rather than the sidebar: the tree follows where the reader
    // went, not only where they arrived.
    const body = await screen.findByRole("main");
    const user = userEvent.setup();
    await user.click(within(body).getByRole("button", { name: "notes" }));
    await user.click(await within(body).findByRole("link", { name: /Beta/ }));

    await waitFor(() => {
      expect(folder).toHaveAttribute("aria-expanded", "true");
    });
    expect(
      await within(nav).findByRole("link", { name: "Beta" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("fades a retired engram in the tree rather than dropping it", async () => {
    serveInDomain();

    renderApp("/d/eng");

    const retired = await within(await sidebar()).findByRole("link", {
      name: "Old Way",
    });
    expect(retired).toBeVisible();
    expect(retired.className).toContain("opacity-60");
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
    // The switcher survives the failure: one domain refusing to be browsed is
    // not a reason to strand somebody in it.
    expect(
      within(await sidebar()).getByRole("button", { name: "Domain: eng" }),
    ).toBeVisible();
  });
});
