/**
 * The frame's own behavior: the domain list it fetches, where the search box
 * sends you, what the theme control writes, and what logging out does.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
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

function serve(routes: Record<string, () => unknown>) {
  apiMock.mockImplementation(answersFor(routes));
}

/** Signed in, with one domain to list. */
function serveSignedIn(extra: Record<string, () => unknown> = {}) {
  serve({
    "/auth/me": () => meResponse({ user: userFixture() }),
    "/domains": domainsResponse,
    ...extra,
  });
}

beforeEach(() => {
  apiMock.mockReset();
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
    serveSignedIn();

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const user = userEvent.setup();
    await user.type(screen.getByLabelText("Search"), "salience{Enter}");

    expect(
      await screen.findByRole("heading", { name: "Search" }),
    ).toBeVisible();
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
        signedIn ? meResponse({ user: userFixture() }) : meResponse(),
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
  });
});
