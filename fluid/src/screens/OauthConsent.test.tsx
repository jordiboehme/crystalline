/**
 * The consent screen: what it shows before a click hands over an account, and
 * where it sends the browser once the click lands.
 *
 * Every assertion here treats the client's name, the redirect host and the
 * account as the whole of what a person needs to decide with - the loopback
 * marker as the one extra fact that changes what "a service" means - and
 * treats navigation itself as the thing to prove, not the page it lands on:
 * `navigateTo` is mocked so the test can see exactly where the screen decided
 * to send the whole page, the way `Profile.test.tsx`'s own SSO tests stop at
 * the request rather than trying to make jsdom follow a redirect.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api, navigateTo } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), navigateTo: vi.fn() };
});

const apiMock = vi.mocked(api);
const navigateToMock = vi.mocked(navigateTo);

function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role: "editor" }) }),
      "/domains": domainsResponse,
      ...routes,
    }),
  );
}

/** One `AuthorizationView`, in whichever state a test needs. */
function viewPayload(overrides: Record<string, unknown> = {}) {
  return {
    client_name: "Claude",
    client_uri: "https://claude.ai",
    redirect_host: "claude.ai",
    loopback: false,
    account: "ada",
    expires_in: 587,
    ...overrides,
  };
}

beforeEach(() => {
  apiMock.mockReset();
  navigateToMock.mockReset();
});

describe("the consent screen", () => {
  it("shows the client, the redirect host and the account, and navigates on allow", async () => {
    serve({
      "/oauth/authorizations/req-1": (_path, init) => {
        if (init?.method === "POST") {
          expect(JSON.parse(init.body as string)).toEqual({
            decision: "allow",
          });
          return {
            location:
              "https://claude.ai/callback?code=abc123&state=xyz&iss=https://kb.example",
          };
        }
        return viewPayload();
      },
    });

    renderApp("/authorize?request=req-1");

    expect(await screen.findByText("Claude")).toBeInTheDocument();
    expect(screen.getByText("claude.ai")).toBeInTheDocument();
    expect(screen.getByText("ada")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Allow" }));

    await waitFor(() => {
      expect(navigateToMock).toHaveBeenCalledWith(
        "https://claude.ai/callback?code=abc123&state=xyz&iss=https://kb.example",
      );
    });
  });

  it("warns on a loopback redirect and navigates on deny", async () => {
    serve({
      "/oauth/authorizations/req-2": (_path, init) => {
        if (init?.method === "POST") {
          expect(JSON.parse(init.body as string)).toEqual({
            decision: "deny",
          });
          return {
            location:
              "http://127.0.0.1:51902/callback?error=access_denied&state=xyz&iss=https://kb.example",
          };
        }
        return viewPayload({
          client_name: "a local agent",
          client_uri: null,
          redirect_host: "127.0.0.1:51902",
          loopback: true,
        });
      },
    });

    renderApp("/authorize?request=req-2");

    expect(await screen.findByText("a local agent")).toBeInTheDocument();
    expect(screen.getByText("127.0.0.1:51902")).toBeInTheDocument();
    expect(screen.getByText(/program on this computer/i)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Deny" }));

    await waitFor(() => {
      expect(navigateToMock).toHaveBeenCalledWith(
        "http://127.0.0.1:51902/callback?error=access_denied&state=xyz&iss=https://kb.example",
      );
    });
  });

  it("says the request expired on a 404", async () => {
    serve({
      "/oauth/authorizations/gone": () => {
        throw new ApiProblem(
          404,
          "not found",
          "no such pending request: it may have expired or already been decided",
        );
      },
    });

    renderApp("/authorize?request=gone");

    expect(
      await screen.findByText(/this request has expired/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/start again from the client/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Allow" }),
    ).not.toBeInTheDocument();
  });

  it("says the request expired when the page carries no request id at all", async () => {
    serve();

    renderApp("/authorize");

    expect(
      await screen.findByText(/this request has expired/i),
    ).toBeInTheDocument();
  });
});
