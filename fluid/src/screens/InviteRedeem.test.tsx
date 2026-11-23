/**
 * The page an invite link opens. The token comes from the fragment only,
 * goes out in POST bodies only, and nothing is redeemed until the person
 * presses a button. Each way in the server names gets its own control.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api, navigateTo, setCsrfToken } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";
import { INVALID_LINK, tokenFromHash } from "./InviteRedeem";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return {
    ...actual,
    api: vi.fn(),
    setCsrfToken: vi.fn(),
    navigateTo: vi.fn(),
  };
});

const apiMock = vi.mocked(api);
const navigateToMock = vi.mocked(navigateTo);
const TOKEN = "cin_secret-token";

function offer(overrides: Record<string, unknown> = {}) {
  return {
    inviter: "olga",
    inviter_display: "Olga Owner",
    domain: "team",
    level: "editor",
    role: "none",
    expires_at: "2026-11-30T12:00:00Z",
    sign_in: "local",
    local_allowed: true,
    ...overrides,
  };
}

function serve(routes: Record<string, Answer> = {}, signedIn = false) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse(signedIn ? { user: userFixture({ name: "ada" }) } : {}),
      "/domains": domainsResponse,
      "/invites/confirm": () => offer(),
      "/invites/redeem": () => ({
        outcome: "redeemed",
        created: true,
        domain: "team",
        level: "editor",
        csrf: "csrf-new",
        user: userFixture({ name: "nina", display: "Nina" }),
      }),
      ...routes,
    }),
  );
}

/** Every request the page sent, path and body. */
function sent(): { path: string; method: string; body: unknown }[] {
  return apiMock.mock.calls.map(([path, init]) => ({
    path,
    method: init?.method ?? "GET",
    body:
      typeof init?.body === "string"
        ? (JSON.parse(init.body) as unknown)
        : undefined,
  }));
}

function bodyOf(path: string): unknown {
  return sent().find((call) => call.path === path && call.method === "POST")
    ?.body;
}

beforeEach(() => {
  apiMock.mockReset();
  navigateToMock.mockReset();
  vi.mocked(setCsrfToken).mockReset();
});

describe("tokenFromHash", () => {
  it("reads the token after #, and nothing from an empty fragment", () => {
    expect(tokenFromHash("#cin_abc")).toBe("cin_abc");
    expect(tokenFromHash("#")).toBeNull();
    expect(tokenFromHash("")).toBeNull();
  });
});

describe("the invite page", () => {
  it("shows what the link offers and redeems nothing on load", async () => {
    serve();
    renderApp(`/invite#${TOKEN}`);
    expect(
      await screen.findByText("Olga Owner (olga)", { exact: true }),
    ).toBeVisible();
    expect(screen.getByText("team", { exact: true })).toBeVisible();
    expect(screen.getByText("Editor", { exact: true })).toBeVisible();
    expect(bodyOf("/invites/confirm")).toEqual({ token: TOKEN });
    expect(sent().some((call) => call.path === "/invites/redeem")).toBe(false);
    // The token never rides in a request address.
    expect(sent().some((call) => call.path.includes(TOKEN))).toBe(false);
  });

  it("says the one sentence for a bad link, and for no token at all", async () => {
    serve({
      "/invites/confirm": () => {
        throw new ApiProblem(404, "not found", INVALID_LINK);
      },
    });
    const { unmount } = renderApp(`/invite#${TOKEN}`);
    expect(await screen.findByRole("alert")).toHaveTextContent(INVALID_LINK);
    unmount();

    apiMock.mockClear();
    renderApp("/invite");
    expect(await screen.findByText(INVALID_LINK)).toBeVisible();
    expect(sent().some((call) => call.path === "/invites/confirm")).toBe(false);
  });

  it("makes a local account and links into the domain", async () => {
    serve();
    renderApp(`/invite#${TOKEN}`);
    await userEvent.type(
      await screen.findByLabelText("Choose a login name"),
      "nina",
    );
    await userEvent.type(screen.getByLabelText("Choose a password"), "pw-nina");
    await userEvent.click(
      screen.getByRole("button", { name: "Create account and join" }),
    );
    expect(await screen.findByRole("status")).toHaveTextContent(
      "You joined team as editor.",
    );
    expect(bodyOf("/invites/redeem")).toEqual({
      token: TOKEN,
      login: "nina",
      password: "pw-nina",
      display_name: null,
    });
    expect(vi.mocked(setCsrfToken)).toHaveBeenCalledWith("csrf-new");
    expect(screen.getByRole("link", { name: "Open team" })).toHaveAttribute(
      "href",
      "/d/team",
    );
  });

  it("signs in to an existing account on the page, then accepts as it", async () => {
    serve({
      "/auth/login": () => ({ csrf: "csrf-login", user: userFixture() }),
    });
    renderApp(`/invite#${TOKEN}`);
    await userEvent.type(await screen.findByLabelText("Name"), "ada");
    await userEvent.type(screen.getByLabelText("Password"), "pw-ada");
    await userEvent.click(
      screen.getByRole("button", { name: "Sign in and join" }),
    );
    expect(await screen.findByRole("status")).toHaveTextContent(
      "You joined team as editor.",
    );
    expect(bodyOf("/auth/login")).toEqual({ name: "ada", password: "pw-ada" });
    expect(bodyOf("/invites/redeem")).toEqual({ token: TOKEN });
    const order = sent().map((call) => call.path);
    expect(order.indexOf("/auth/login")).toBeLessThan(
      order.indexOf("/invites/redeem"),
    );
  });

  it("accepts for the account already signed in", async () => {
    serve({}, true);
    renderApp(`/invite#${TOKEN}`);
    expect(
      await screen.findByText("You are signed in as Ada Lovelace (ada)."),
    ).toBeVisible();
    expect(screen.queryByLabelText("Choose a login name")).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Accept invite" }),
    );
    await waitFor(() => {
      expect(bodyOf("/invites/redeem")).toEqual({ token: TOKEN });
    });
    expect(await screen.findByRole("status")).toHaveTextContent(
      "You joined team",
    );
  });

  it("starts the single sign-on and says where it lands", async () => {
    serve({
      "/invites/confirm": () =>
        offer({ sign_in: "oidc", local_allowed: false }),
      "/invites/redeem": () => ({
        outcome: "sign_in",
        created: false,
        location: "https://idp.example/authorize?state=x",
      }),
    });
    renderApp(`/invite#${TOKEN}`);
    expect(
      await screen.findByText("You come back signed in, on the page of team."),
    ).toBeVisible();
    expect(screen.queryByLabelText("Choose a login name")).toBeNull();
    await userEvent.click(
      screen.getByRole("button", { name: "Continue with single sign-on" }),
    );
    await waitFor(() => {
      expect(navigateToMock).toHaveBeenCalledWith(
        "https://idp.example/authorize?state=x",
      );
    });
    expect(bodyOf("/invites/redeem")).toEqual({ token: TOKEN });
  });

  it("shows the server's sentence when the redemption is refused", async () => {
    serve({
      "/invites/redeem": () => {
        throw new ApiProblem(410, "gone", "this invite link was used already");
      },
    });
    renderApp(`/invite#${TOKEN}`);
    await userEvent.type(
      await screen.findByLabelText("Choose a login name"),
      "nina",
    );
    await userEvent.type(screen.getByLabelText("Choose a password"), "pw");
    await userEvent.click(
      screen.getByRole("button", { name: "Create account and join" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "this invite link was used already",
    );
  });
});
