/**
 * The GitHub settings screen.
 *
 * The only settings section this app ships, and the one screen where a
 * credential changes hands. What is pinned here is what an admin has to be
 * able to trust: that it exists for nobody else, that both ways in are on
 * offer, that the device flow shows the code and where to type it, that a
 * token typed in is sent once and left nowhere, and that every refusal is the
 * server's own sentence rather than a house message pasted over it.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
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

/** The wire shape of a status, in whichever of its states a test needs. */
function statusPayload(overrides: Record<string, unknown> = {}) {
  return {
    enabled: false,
    connected: false,
    user: null,
    token_store: null,
    pending: null,
    error: null,
    ...overrides,
  };
}

/** A device flow waiting for its browser half. */
const PENDING = {
  user_code: "ABCD-1234",
  verification_url: "https://github.example/device",
  expires_in_secs: 900,
};

/** The app, signed in as `root` in the given role. */
function serveAs(
  role: "admin" | "editor",
  routes: Record<string, Answer> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "root", role }) }),
      "/domains": domainsResponse,
      "/settings/github": () => statusPayload(),
      ...routes,
    }),
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

/** Every call the app made to a settings route. */
function settingsCalls(): unknown[] {
  return apiMock.mock.calls.filter(([path]) =>
    String(path).startsWith("/settings/github"),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the GitHub settings screen", () => {
  it("is not there for a non-admin", async () => {
    serveAs("editor");
    renderApp("/settings/github");

    expect(await screen.findByText(/nothing here/i)).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "GitHub" }),
    ).not.toBeInTheDocument();
    // The screen asks the server nothing, and the frame advertises no door
    // this session cannot open.
    expect(settingsCalls()).toHaveLength(0);
    expect(
      screen.queryByRole("link", { name: /github/i }),
    ).not.toBeInTheDocument();
  });

  it("offers the nav entry to an admin", async () => {
    serveAs("admin");
    renderApp("/settings/github");

    expect(await screen.findByRole("link", { name: "GitHub" })).toHaveAttribute(
      "href",
      "/settings/github",
    );
  });

  it("shows the disconnected state with both connect paths", async () => {
    serveAs("admin");
    renderApp("/settings/github");

    expect(
      await screen.findByRole("heading", { name: "GitHub" }),
    ).toBeInTheDocument();
    expect(await screen.findByText(/not connected/i)).toBeInTheDocument();
    // Both ways in, side by side: the browser sign-in, and a token for an
    // instance nobody is sitting in front of.
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
    let status = statusPayload();
    serveAs("admin", {
      "/settings/github": () => status,
      "/settings/github/connect": () => {
        status = statusPayload({ enabled: true, pending: PENDING });
        return status;
      },
    });
    renderApp("/settings/github");

    await userEvent.click(
      await screen.findByRole("button", { name: /connect with github/i }),
    );

    // The code, where to type it, and the fact that the app is now waiting on
    // a browser somebody has to visit.
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

  it("keeps a finished flow's failure quiet while a fresh one runs", async () => {
    // The connect answer may carry the PREVIOUS flow's once-reported failure
    // alongside the new pending block. The fresh flow is what the screen is
    // about, so the stale sentence waits.
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          pending: PENDING,
          error: "the code expired before it was confirmed",
        }),
    });
    renderApp("/settings/github");

    expect(await screen.findByText("ABCD-1234")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("submits a PAT and clears the field", async () => {
    let status = statusPayload();
    serveAs("admin", {
      "/settings/github": () => status,
      "/settings/github/token": () => {
        status = statusPayload({
          enabled: true,
          connected: true,
          user: "octo",
          token_store: "keyring",
        });
        return status;
      },
    });
    renderApp("/settings/github");

    const field = await screen.findByLabelText(/personal access token/i);
    await userEvent.type(field, "ghp_secret");
    await userEvent.click(
      screen.getByRole("button", { name: /connect with token/i }),
    );

    await waitFor(() => {
      expect(sentBody("/settings/github/token", "POST")).toEqual({
        token: "ghp_secret",
      });
    });
    // The field empties only once the server took it, and the card then says
    // whose credential is on file and where it lives.
    await waitFor(() => {
      expect(field).toHaveValue("");
    });
    expect(await screen.findByText(/connected as octo/i)).toBeInTheDocument();
    expect(screen.getByText(/keyring/i)).toBeInTheDocument();
  });

  it("keeps the token when the server refuses it", async () => {
    serveAs("admin", {
      "/settings/github/token": () => {
        throw new ApiProblem(
          422,
          "unprocessable entity",
          "GitHub refused that token: it may be expired or lack the repo scope",
        );
      },
    });
    renderApp("/settings/github");

    const field = await screen.findByLabelText(/personal access token/i);
    await userEvent.type(field, "ghp_stale");
    await userEvent.click(
      screen.getByRole("button", { name: /connect with token/i }),
    );

    expect(await screen.findByText(/lack the repo scope/i)).toBeInTheDocument();
    expect(field).toHaveValue("ghp_stale");
  });

  it("surfaces a device-flow failure in the server's words", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({ enabled: true, error: "authorization denied" }),
    });
    renderApp("/settings/github");

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/authorization denied/i);
  });

  it("disconnects behind a two-step confirm", async () => {
    let status = statusPayload({
      enabled: true,
      connected: true,
      user: "octo",
      token_store: "keyring",
    });
    const forgotten = vi.fn(() => {
      status = statusPayload({ enabled: true });
      return status;
    });
    serveAs("admin", {
      "/settings/github": (_path, init) =>
        init?.method === "DELETE" ? forgotten() : status,
    });
    renderApp("/settings/github");

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

  it("is reachable from the command palette", async () => {
    serveAs("admin", { "/activity": () => ({ timeframe: "7d", items: [] }) });
    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    await userEvent.keyboard("{Meta>}k{/Meta}");
    await userEvent.click(
      await screen.findByRole("option", { name: /github settings/i }),
    );

    expect(
      await screen.findByRole("heading", { name: "GitHub" }),
    ).toBeInTheDocument();
  });
});
