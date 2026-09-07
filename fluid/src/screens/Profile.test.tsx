/**
 * The profile screen, which is two cards: the GitHub identity this account
 * shares as, and the MCP tokens an agent authenticates the daemon with, acting
 * as this account.
 *
 * The GitHub half pins what somebody about to share has to be able to trust:
 * that both ways in are on offer, that the device flow shows the code and where
 * to type it, that a token typed in is sent once and left nowhere, that the
 * connected card names the account and since when, and that a refusal - a
 * viewer's, or a sign-in somebody else already started - is the server's own
 * sentence rather than a house message pasted over it.
 *
 * The agent access half pins the opposite direction: that the card is offered
 * to a viewer exactly as it is to an editor and never gated by an instance's
 * read-only setting, that a freshly issued or rotated secret is shown exactly
 * once and gone from the DOM the moment its dialog is dismissed, that the
 * listing never carries the secret at all, that every one of its four failure
 * surfaces shows the server's own words, that a revoke abandoned by Escape or
 * Keep hands focus back to the row's own Revoke button, and that a failed
 * issue is never retried into a second, unseen token.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
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
import { Tooltips } from "../components/primitives";
import { AgentAccessCard } from "./Profile";

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
      "/me/mcp-tokens": () => [],
      // The default is an instance with no provider and an account with no
      // link, which is the shape in which the SSO card is not there at all.
      "/auth/providers": () => ({ local: true, oidc: { enabled: false } }),
      "/me/identity-links": () => ({ links: [], has_password: true }),
      ...routes,
    }),
  );
}

/** An instance with a provider configured, for the SSO card's own tests. */
function withProvider(routes: Record<string, Answer> = {}) {
  return {
    "/auth/providers": () => ({
      local: true,
      oidc: { enabled: true, name: "Contoso" },
    }),
    ...routes,
  };
}

/** One link, as the listing hands it back. */
function linkPayload(overrides: Record<string, unknown> = {}) {
  return {
    issuer: "https://idp.example",
    subject: "sub-1",
    linked_at: "2026-09-01T09:12:44Z",
    linked_by: "ada",
    ...overrides,
  };
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

  /**
   * The half the flow's own screen cannot see: the confirmation happens in
   * another window, so nothing tells this card it happened and the poll is
   * what learns it. No click anywhere - the card is rendered already waiting,
   * which is the state the server reports to a browser that reloaded mid-flow
   * as much as to the one that started it.
   *
   * On the real clock, and the generous timeouts are why: the card polls every
   * three seconds, and testing-library's waiter does not recognise vitest's
   * fake clock (it looks for jest's), so a faked one would never be advanced
   * by the wait and the test would hang rather than run fast. One real poll
   * interval is the price of covering the one thing this card does that
   * nothing else can cover for it.
   */
  it("polls a pending sign-in through to connected", async () => {
    let identity: Record<string, unknown> = identityPayload({
      pending: PENDING,
    });
    serveAs("editor", { "/me/github-identity": () => identity });
    renderApp("/profile");

    expect(await screen.findByText("ABCD-1234")).toBeInTheDocument();

    // Confirmed in the other window.
    identity = CONNECTED;

    expect(
      await screen.findByText(/connected as @octo/i, {}, { timeout: 8000 }),
    ).toBeInTheDocument();
    expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument();
    expect(
      screen.queryByText(/waiting for the browser confirmation/i),
    ).not.toBeInTheDocument();
  }, 15000);

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

describe("the agent access card", () => {
  it("lists the caller's own tokens, never a secret", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => [
        {
          id: 1,
          label: "laptop",
          created_at: "2026-08-29T09:12:44Z",
          last_used: "2026-09-01T10:00:00Z",
        },
        {
          id: 2,
          label: "ci",
          created_at: "2026-08-20T00:00:00Z",
          last_used: null,
        },
      ],
    });
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "Agent access" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("laptop")).toBeInTheDocument();
    expect(screen.getByText("2026-08-29")).toBeInTheDocument();
    expect(screen.getByText("ci")).toBeInTheDocument();
    expect(screen.getByText("Never")).toBeInTheDocument();
    expect(screen.queryByText(/cmt_/)).not.toBeInTheDocument();
  });

  it("says so when no token has been issued yet", async () => {
    serveAs("viewer");
    renderApp("/profile");

    expect(
      await screen.findByText(/no tokens issued yet/i),
    ).toBeInTheDocument();
  });

  it("is offered to a viewer too, since an agent acts as its user", async () => {
    serveAs("viewer");
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "Agent access" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Issue token" }),
    ).toBeInTheDocument();
  });

  it("issues a token, reveals it exactly once, and never again after it is dismissed", async () => {
    let tokens: Record<string, unknown>[] = [];
    serveAs("editor", {
      "/me/mcp-tokens": (_path, init) => {
        if (init?.method === "POST") {
          tokens = [
            {
              id: 9,
              label: "laptop",
              created_at: "2026-09-07T00:00:00Z",
              last_used: null,
            },
          ];
          return { id: 9, label: "laptop", token: "cmt_deadbeef" };
        }
        return tokens;
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText("Label");
    await userEvent.type(field, "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));

    await waitFor(() => {
      expect(sentBody("/me/mcp-tokens", "POST")).toEqual({ label: "laptop" });
    });
    // The field clears once the server took the label, the same rule the
    // GitHub token field above follows.
    await waitFor(() => {
      expect(field).toHaveValue("");
    });

    const dialog = await screen.findByRole("dialog", { name: "laptop" });
    expect(within(dialog).getByText("cmt_deadbeef")).toBeInTheDocument();
    expect(
      within(dialog).getByText(
        (_text, node) =>
          node?.textContent ===
          "Add this as header Authorization: Bearer cmt_deadbeef to the crystalline entry in your agent's MCP registration.",
      ),
    ).toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    // Dismissed, and gone from the DOM for good - the secret is held nowhere
    // this screen could show it back from.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByText("cmt_deadbeef")).not.toBeInTheDocument();
    expect(await screen.findByText("laptop")).toBeInTheDocument();
  });

  it("rotates a token and reveals the fresh secret", async () => {
    const listing = [
      {
        id: 3,
        label: "laptop",
        created_at: "2026-08-01T00:00:00Z",
        last_used: null,
      },
    ];
    serveAs("editor", {
      "/me/mcp-tokens": () => listing,
      "/me/mcp-tokens/3/rotate": () => ({
        id: 3,
        label: "laptop",
        token: "cmt_freshbeef",
      }),
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Rotate laptop" }),
    );

    const dialog = await screen.findByRole("dialog", { name: "laptop" });
    expect(within(dialog).getByText("cmt_freshbeef")).toBeInTheDocument();
  });

  it("revokes a token behind a two-step confirm", async () => {
    let listing = [
      {
        id: 5,
        label: "laptop",
        created_at: "2026-08-01T00:00:00Z",
        last_used: null,
      },
    ];
    const revoked = vi.fn(() => {
      listing = [];
    });
    serveAs("editor", {
      "/me/mcp-tokens": () => listing,
      "/me/mcp-tokens/5": (_path, init) => {
        if (init?.method === "DELETE") {
          revoked();
        }
        return undefined;
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Revoke laptop" }),
    );
    expect(revoked).not.toHaveBeenCalled();

    await userEvent.click(
      screen.getByRole("button", { name: "Confirm revoke laptop" }),
    );
    await waitFor(() => {
      expect(revoked).toHaveBeenCalled();
    });
    expect(
      await screen.findByText(/no tokens issued yet/i),
    ).toBeInTheDocument();
  });

  it("offers issue, rotate and revoke on a read-only instance too, since a token is account state rather than knowledge", async () => {
    serveAs(
      "editor",
      {
        "/me/mcp-tokens": () => [
          {
            id: 1,
            label: "laptop",
            created_at: "2026-08-01T00:00:00Z",
            last_used: null,
          },
        ],
      },
      { read_only: true },
    );
    renderApp("/profile");

    // The read-only setting protects the knowledge base; it says nothing
    // about a token, which is unrelated to it - every control here is drawn.
    expect(await screen.findByText("laptop")).toBeInTheDocument();
    expect(screen.getByLabelText("Label")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Issue token" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Rotate laptop" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Revoke laptop" }),
    ).toBeInTheDocument();
  });

  it("shows the server's own words when issuing a token is refused", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": (_path, init) => {
        if (init?.method === "POST") {
          throw new ApiProblem(
            422,
            "unprocessable entity",
            "you already hold the maximum number of tokens",
          );
        }
        return [];
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText("Label");
    await userEvent.type(field, "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));

    expect(
      await screen.findByText(/you already hold the maximum/i),
    ).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the server's own words when rotating a token is refused", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => [
        {
          id: 4,
          label: "laptop",
          created_at: "2026-08-01T00:00:00Z",
          last_used: null,
        },
      ],
      "/me/mcp-tokens/4/rotate": () => {
        throw new ApiProblem(
          404,
          "not found",
          "no such MCP token: it may already have been revoked",
        );
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Rotate laptop" }),
    );

    expect(await screen.findByText(/no such mcp token/i)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the server's own words when revoking a token is refused", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => [
        {
          id: 6,
          label: "laptop",
          created_at: "2026-08-01T00:00:00Z",
          last_used: null,
        },
      ],
      "/me/mcp-tokens/6": (_path, init) => {
        if (init?.method === "DELETE") {
          throw new ApiProblem(
            404,
            "not found",
            "no such MCP token: it may already have been revoked",
          );
        }
        return undefined;
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Revoke laptop" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm revoke laptop" }),
    );

    expect(await screen.findByText(/no such mcp token/i)).toBeInTheDocument();
    // Refused, so the row nothing happened to is still there.
    expect(screen.getByText("laptop")).toBeInTheDocument();
  });

  it("shows the server's own words when the listing itself fails", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => {
        throw new ApiProblem(
          500,
          "internal error",
          "the account store could not be read",
        );
      },
    });
    renderApp("/profile");

    expect(
      await screen.findByText(/the account store could not be read/i),
    ).toBeInTheDocument();
  });

  it("returns focus to the row's own Revoke button after Escape and after Keep", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => [
        {
          id: 7,
          label: "laptop",
          created_at: "2026-08-01T00:00:00Z",
          last_used: null,
        },
      ],
    });
    renderApp("/profile");

    const trigger = await screen.findByRole("button", {
      name: "Revoke laptop",
    });

    await userEvent.click(trigger);
    expect(
      screen.getByRole("button", { name: "Confirm revoke laptop" }),
    ).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(
      screen.queryByRole("button", { name: "Confirm revoke laptop" }),
    ).not.toBeInTheDocument();
    // The trigger is rendered unconditionally for exactly this: its ref stays
    // live while confirming, so abandoning has something real to focus.
    expect(trigger).toHaveFocus();

    await userEvent.click(trigger);
    await userEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(trigger).toHaveFocus();
  });

  /**
   * The client's default retries once, after roughly a second, for anything
   * that is not a 4xx - real time, since testing-library's waiter does not
   * recognise vitest's fake clock. `retry: false` on `issue` is what keeps a
   * dropped connection from minting a second, unseen token; this fails
   * without it.
   */
  it("does not retry a failed issue, so a dropped connection never mints a second token", async () => {
    const attempts = vi.fn();
    serveAs("editor", {
      "/me/mcp-tokens": (_path, init) => {
        if (init?.method === "POST") {
          attempts();
          throw new ApiProblem(
            0,
            "network error",
            "could not reach the server: it may be down",
          );
        }
        return [];
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText("Label");
    await userEvent.type(field, "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));

    expect(
      await screen.findByText(/could not reach the server/i),
    ).toBeInTheDocument();
    expect(attempts).toHaveBeenCalledTimes(1);

    await new Promise((resolve) => setTimeout(resolve, 1500));
    expect(attempts).toHaveBeenCalledTimes(1);
  }, 8000);

  it("never lets the issued secret become a value React Query itself retains", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/me/mcp-tokens": (_path, init) => {
          if (init?.method === "POST") {
            return { id: 11, label: "laptop", token: "cmt_deadbeef" };
          }
          return [];
        },
      }),
    );
    const client = new QueryClient({
      defaultOptions: { mutations: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <Tooltips>
          <AgentAccessCard />
        </Tooltips>
      </QueryClientProvider>,
    );

    const field = await screen.findByLabelText("Label");
    await userEvent.type(field, "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));
    await screen.findByRole("dialog", { name: "laptop" });

    // The mutation cache is inspected directly - not the DOM - because the
    // point is what React Query itself retains, which the screen could not
    // reveal either way.
    expect(JSON.stringify(client.getMutationCache().getAll())).not.toContain(
      "cmt_deadbeef",
    );
  });
});

describe("the SSO identity card", () => {
  it("is absent when there is no provider and no identity to show", async () => {
    serveAs("editor");
    renderApp("/profile");

    // The GitHub card is the marker that the screen finished rendering, so
    // the absence below is an absence rather than a race.
    await screen.findByRole("heading", { name: "GitHub identity" });
    expect(
      screen.queryByRole("heading", { name: "SSO identity" }),
    ).not.toBeInTheDocument();
  });

  it("offers the link as a whole-page navigation carrying the link flag", async () => {
    serveAs("editor", withProvider());
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "SSO identity" }),
    ).toBeInTheDocument();
    expect(
      await screen.findByText(/no provider identity linked/i),
    ).toBeInTheDocument();
    // A link, not a button: the sign-on redirects to the provider and back,
    // and `link=true` is what makes it link rather than sign somebody in as
    // whoever the identity turns out to be.
    expect(screen.getByRole("link", { name: "Link Contoso" })).toHaveAttribute(
      "href",
      "/api/v1/auth/oidc/login?link=true",
    );
  });

  it("shows a linked identity, who linked it, and unlinks it", async () => {
    let links = [linkPayload({ linked_by: "jit" })];
    serveAs(
      "editor",
      withProvider({
        "/me/identity-links": () => ({ links, has_password: true }),
        "/me/identity-links/https%3A%2F%2Fidp.example": () => {
          links = [];
          return undefined;
        },
      }),
    );
    renderApp("/profile");

    expect(await screen.findByText("https://idp.example")).toBeInTheDocument();
    expect(screen.getByText(/created by a first sign-on/i)).toBeInTheDocument();
    // With one identity already held, there is nothing to link: one identity
    // per provider is the rule.
    expect(
      screen.queryByRole("link", { name: "Link Contoso" }),
    ).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Unlink" }));

    expect(await screen.findByRole("status")).toHaveTextContent(
      "The identity is unlinked.",
    );
    expect(
      await screen.findByText(/no provider identity linked/i),
    ).toBeInTheDocument();
  });

  it("shows the server's own words when the last way in cannot be given up", async () => {
    serveAs(
      "editor",
      withProvider({
        "/me/identity-links": () => ({
          links: [linkPayload({ linked_by: "jit" })],
          has_password: false,
        }),
        "/me/identity-links/https%3A%2F%2Fidp.example": () => {
          throw new ApiProblem(
            409,
            "conflict",
            "this is the only way into account 'ada': it has no password, so unlinking " +
              "its last identity would leave nobody able to sign in - give it a password " +
              "first with `crystalline users passwd ada`",
          );
        },
      }),
    );
    renderApp("/profile");

    // The card says so before anybody presses anything, and the refusal says
    // it again in the server's own sentence.
    expect(
      await screen.findByText(/this identity is its only way in/i),
    ).toBeInTheDocument();

    await userEvent.click(
      await screen.findByRole("button", { name: "Unlink" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "crystalline users passwd ada",
    );
    expect(screen.getByText("https://idp.example")).toBeInTheDocument();
  });

  it("keeps showing a link whose provider was turned off, so it can be given up", async () => {
    serveAs("editor", {
      "/me/identity-links": () => ({
        links: [linkPayload({ linked_by: "cli" })],
        has_password: true,
      }),
    });
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "SSO identity" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/linked by an administrator/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Unlink" })).toBeInTheDocument();
  });
});
