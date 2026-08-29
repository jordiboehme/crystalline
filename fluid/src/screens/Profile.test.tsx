/**
 * The profile screen, which is one card: the GitHub identity this account
 * shares as.
 *
 * What is pinned here is what somebody about to share has to be able to trust:
 * that both ways in are on offer, that the device flow shows the code and where
 * to type it, that a token typed in is sent once and left nowhere, that the
 * connected card names the account and since when, and that a refusal - a
 * viewer's, or a sign-in somebody else already started - is the server's own
 * sentence rather than a house message pasted over it.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { MeResponse } from "../api/model";
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
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** The wire shape of an identity, in whichever state a test needs. */
function identityPayload(overrides: Record<string, unknown> = {}) {
  return {
    account: "ada",
    connected: false,
    login: null,
    connected_at: null,
    token_store: null,
    pending: null,
    error: null,
    ...overrides,
  };
}

/** The identity as it stands once a credential is on file. */
const CONNECTED = identityPayload({
  connected: true,
  login: "octo",
  connected_at: "2026-08-29T09:12:44Z",
  token_store: "keyring",
});

/** A device flow waiting for its browser half. */
const PENDING = {
  user_code: "ABCD-1234",
  verification_url: "https://github.example/device",
  expires_in_secs: 900,
};

/** The app, signed in as `ada` in the given role. */
function serveAs(
  role: "admin" | "editor" | "viewer",
  routes: Record<string, Answer> = {},
  me: Partial<MeResponse> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }), ...me }),
      "/domains": domainsResponse,
      "/me/github-identity": () => identityPayload(),
      ...routes,
    }),
  );
}

/** Every call the app made to the personal identity surface. */
function identityCalls(): unknown[] {
  return apiMock.mock.calls.filter(([path]) =>
    String(path).startsWith("/me/github-identity"),
  );
}

/** The body of the request the app sent to `path` with `method`, parsed. */
function sentBody(path: string, method: string): unknown {
  const call = apiMock.mock.calls.find(
    ([sent, init]) => sent === path && init?.method === method,
  );
  if (!call) {
    throw new Error(`no ${method} to ${path}`);
  }
  const body = call[1]?.body;
  if (typeof body !== "string") {
    throw new Error(`the ${method} to ${path} carried no JSON body`);
  }
  return JSON.parse(body) as unknown;
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the profile screen", () => {
  it("is offered to every signed-in account, inside the identity menu", async () => {
    serveAs("editor");
    renderApp("/profile");

    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Ada Lovelace" }),
    );
    expect(
      await screen.findByRole("menuitem", { name: "Profile" }),
    ).toHaveAttribute("href", "/profile");
  });

  it("shows the disconnected card with both connect paths", async () => {
    serveAs("editor");
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "GitHub identity" }),
    ).toBeInTheDocument();
    expect(await screen.findByText(/not connected/i)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /connect with github/i }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/personal access token/i)).toHaveAttribute(
      "type",
      "password",
    );
    expect(
      screen.getByRole("button", { name: /connect with token/i }),
    ).toBeInTheDocument();
  });

  it("starts the device flow and shows code, link and polling state", async () => {
    let identity = identityPayload();
    serveAs("editor", {
      "/me/github-identity": () => identity,
      "/me/github-identity/connect": () => {
        identity = identityPayload({ pending: PENDING });
        return identity;
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: /connect with github/i }),
    );

    expect(await screen.findByText("ABCD-1234")).toBeInTheDocument();
    const link = screen.getByRole("link", {
      name: /open github\.com and enter the code/i,
    });
    expect(link).toHaveAttribute("href", "https://github.example/device");
    expect(link).toHaveAttribute("target", "_blank");
    expect(
      screen.getByText(/waiting for the browser confirmation/i),
    ).toBeInTheDocument();
  });

  it("surfaces a sign-in somebody else already started, in the server's words", async () => {
    serveAs("editor", {
      "/me/github-identity/connect": () => {
        throw new ApiProblem(
          409,
          "conflict",
          "another sign-in is in progress on this instance: wait for it to finish, then start yours again",
        );
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: /connect with github/i }),
    );

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/another sign-in is in progress/i);
    // And the card stays where it was: nothing to type, nothing to confirm.
    expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument();
  });

  it("pastes a token and shows the identity it stored", async () => {
    let identity: Record<string, unknown> = identityPayload();
    serveAs("editor", {
      "/me/github-identity": (_path, init) => {
        if (init?.method === "PUT") {
          throw new Error("the token goes to its own route");
        }
        return identity;
      },
      "/me/github-identity/token": () => {
        identity = CONNECTED;
        return identity;
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText(/personal access token/i);
    await userEvent.type(field, "ghp_secret");
    await userEvent.click(
      screen.getByRole("button", { name: /connect with token/i }),
    );

    await waitFor(() => {
      expect(sentBody("/me/github-identity/token", "PUT")).toEqual({
        token: "ghp_secret",
      });
    });
    // The field empties only once the server took it, and the card then names
    // the account, since when, and where the credential lives.
    await waitFor(() => {
      expect(field).toHaveValue("");
    });
    expect(await screen.findByText(/connected as @octo/i)).toBeInTheDocument();
    expect(screen.getByText(/since 2026-08-29/i)).toHaveTextContent(/keyring/i);
  });

  it("keeps the token when the server refuses it", async () => {
    serveAs("editor", {
      "/me/github-identity/token": () => {
        throw new ApiProblem(
          422,
          "unprocessable entity",
          "GitHub refused that token: it may be expired or lack the repo scope",
        );
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText(/personal access token/i);
    await userEvent.type(field, "ghp_stale");
    await userEvent.click(
      screen.getByRole("button", { name: /connect with token/i }),
    );

    expect(await screen.findByText(/lack the repo scope/i)).toBeInTheDocument();
    expect(field).toHaveValue("ghp_stale");
  });

  it("disconnects behind a two-step confirm", async () => {
    let identity: Record<string, unknown> = CONNECTED;
    const forgotten = vi.fn(() => {
      identity = identityPayload();
      return identity;
    });
    serveAs("editor", {
      "/me/github-identity": (_path, init) =>
        init?.method === "DELETE" ? forgotten() : identity,
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Disconnect" }),
    );
    expect(forgotten).not.toHaveBeenCalled();

    await userEvent.click(
      screen.getByRole("button", { name: /confirm disconnect/i }),
    );
    await waitFor(() => {
      expect(forgotten).toHaveBeenCalled();
    });
    expect(await screen.findByText(/not connected/i)).toBeInTheDocument();
  });

  it("tells a viewer that sharing is not theirs, and asks the server nothing", async () => {
    serveAs("viewer");
    renderApp("/profile");

    expect(
      await screen.findByText(/sharing is not available for viewer accounts/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /connect with github/i }),
    ).not.toBeInTheDocument();
    // The route refuses a viewer anyway, so the card does not go and ask.
    expect(identityCalls()).toHaveLength(0);
  });

  it("offers no way to connect on a read-only instance", async () => {
    serveAs(
      "editor",
      { "/me/github-identity": () => CONNECTED },
      { read_only: true },
    );
    renderApp("/profile");

    // The read still works, so the card says what is on file.
    expect(await screen.findByText(/connected as @octo/i)).toBeInTheDocument();
    // Every verb that would change it is refused by the server, so none of
    // them is offered: the app draws no door that will not open.
    expect(
      screen.queryByRole("button", { name: /connect with github/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/personal access token/i),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Disconnect" }),
    ).not.toBeInTheDocument();
    expect(
      await screen.findByText(/nothing here can be connected or disconnected/i),
    ).toBeInTheDocument();
  });
});
