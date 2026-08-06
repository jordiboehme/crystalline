/**
 * The frame's own behavior: the domain list it fetches, where the search box
 * sends you, what the theme control writes, and what logging out does.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api, setCsrfToken } from "../api/client";
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

function serve(routes: Record<string, () => unknown>) {
  apiMock.mockImplementation(answersFor(routes));
}

/** Signed in, with one domain to list and a quiet activity feed. */
function serveSignedIn(extra: Record<string, () => unknown> = {}) {
  serve({
    "/auth/me": () => meResponse({ user: userFixture() }),
    "/domains": domainsResponse,
    // The home screen behind this frame reads the feed; an unstubbed route
    // would fail and put a second alert on screen.
    "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
    ...extra,
  });
}

beforeEach(() => {
  apiMock.mockReset();
  setCsrfTokenMock.mockReset();
  document.documentElement.removeAttribute("data-theme");
});

describe("the layout", () => {
  it("lists the instance's domains in the sidebar", async () => {
    serveSignedIn();

    renderApp("/");

    const domains = await screen.findByRole("navigation", { name: "Domains" });
    const link = await within(domains).findByRole("link", { name: /eng/ });
    expect(link).toHaveAttribute("href", "/d/eng");
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
        order: apiMock.mock.invocationCallOrder[index],
      }))
      .filter((call) => call.path === "/auth/me");
    expect(droppedAt).toBeLessThan(probes[probes.length - 1].order);
  });
});
