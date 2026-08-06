/**
 * The bootstrap probe decides which app you get, and every answer it can give
 * has to land somewhere deliberate. These tests walk the whole set: an account,
 * no identity, a refusal, the anonymous viewer, a disabled account, and a
 * server that is not there at all.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api, setCsrfToken } from "../api/client";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

// Only the transport is replaced. `ApiProblem` and the encoders stay real, so
// a test failing on a status is failing on the app's reading of it.
vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);
const setCsrfTokenMock = vi.mocked(setCsrfToken);

/** Point the mocked client at a path-to-answer table. */
function serve(routes: Record<string, () => unknown>) {
  apiMock.mockImplementation(answersFor(routes));
}

beforeEach(() => {
  apiMock.mockReset();
  setCsrfTokenMock.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("the bootstrap probe", () => {
  it("renders the app when it returns an account", async () => {
    serve({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
    });

    renderApp("/");

    expect(await screen.findByRole("heading", { name: "Home" })).toBeVisible();
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
  });

  it("redirects to the login screen when it returns no identity", async () => {
    serve({ "/auth/me": () => meResponse() });

    renderApp("/");

    expect(await screen.findByLabelText("Name")).toBeVisible();
    expect(screen.getByLabelText("Password")).toBeVisible();
  });

  it("redirects to the login screen when it is refused with a 401", async () => {
    serve({
      "/auth/me": () => {
        throw new ApiProblem(401, "unauthorized", "log in first");
      },
    });

    renderApp("/");

    expect(await screen.findByLabelText("Name")).toBeVisible();
  });

  it("enters as an anonymous viewer, with no redirect and a named menu", async () => {
    serve({
      "/auth/me": () => meResponse({ anonymous: true }),
      "/domains": domainsResponse,
    });

    renderApp("/");

    expect(await screen.findByRole("heading", { name: "Home" })).toBeVisible();
    expect(
      screen.getByRole("button", { name: /Viewing anonymously/ }),
    ).toBeVisible();
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
  });

  it("shows the account-disabled screen on a 403, never the login form", async () => {
    serve({
      "/auth/me": () => {
        throw new ApiProblem(403, "forbidden", "this account is disabled");
      },
    });

    renderApp("/");

    expect(
      await screen.findByRole("heading", { name: /account is disabled/i }),
    ).toBeVisible();
    expect(screen.getByText("this account is disabled")).toBeVisible();
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
  });

  it("shows the server-down banner when the server cannot be reached", async () => {
    serve({
      "/auth/me": () => {
        throw new ApiProblem(0, "network error", "could not reach the server");
      },
    });

    renderApp("/");

    // A request that never arrived is the one failure worth trying again, so
    // the banner is a retry away rather than immediate.
    expect(
      await screen.findByRole(
        "heading",
        { name: /cannot reach Crystalline/i },
        { timeout: 5000 },
      ),
    ).toBeVisible();
    expect(screen.getByText("could not reach the server")).toBeVisible();
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
  });

  it("keeps a decided refusal decided rather than retrying it", async () => {
    serve({
      "/auth/me": () => {
        throw new ApiProblem(403, "forbidden", "this account is disabled");
      },
    });

    renderApp("/");

    await screen.findByRole("heading", { name: /account is disabled/i });
    expect(apiMock).toHaveBeenCalledTimes(1);
  });

  it("feeds the token it hands out to the client", async () => {
    serve({
      "/auth/me": () => meResponse({ user: userFixture(), csrf: "probe-tok" }),
      "/domains": domainsResponse,
    });

    renderApp("/");

    await screen.findByRole("heading", { name: "Home" });
    expect(setCsrfTokenMock).toHaveBeenCalledWith("probe-tok");
  });

  it("forgets the token when the probe hands none out", async () => {
    serve({
      "/auth/me": () => meResponse({ anonymous: true }),
      "/domains": domainsResponse,
    });

    renderApp("/");

    await screen.findByRole("heading", { name: "Home" });
    expect(setCsrfTokenMock).toHaveBeenCalledWith(null);
  });

  it("warns when the server is a different version than this build", async () => {
    serve({
      "/auth/me": () =>
        meResponse({ user: userFixture(), version: "99.9.9-other" }),
      "/domains": domainsResponse,
    });

    renderApp("/");

    // A Radix toast also renders the same words into an announcement region
    // for screen readers, so both copies are expected and either will do.
    const [warning] = await screen.findAllByText(
      `Fluid ${import.meta.env.VITE_APP_VERSION} is talking to Crystalline 99.9.9-other`,
    );
    expect(warning).toBeVisible();
    expect(screen.getByRole("button", { name: "Dismiss" })).toBeVisible();
  });

  it("lets the version warning be dismissed", async () => {
    serve({
      "/auth/me": () =>
        meResponse({ user: userFixture(), version: "99.9.9-other" }),
      "/domains": domainsResponse,
    });

    renderApp("/");

    await userEvent.click(
      await screen.findByRole("button", { name: "Dismiss" }),
    );
    await waitFor(() => {
      expect(
        screen.queryByText(/is talking to Crystalline/),
      ).not.toBeInTheDocument();
    });
  });

  it("says nothing about versions when they agree", async () => {
    serve({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
    });

    renderApp("/");

    await screen.findByRole("heading", { name: "Home" });
    expect(
      screen.queryByText(/is talking to Crystalline/),
    ).not.toBeInTheDocument();
  });
});

describe("a probe that fails after the app is already up", () => {
  it("drops a stale identity when it is refused", async () => {
    let probes = 0;
    serve({
      "/auth/me": () => {
        probes += 1;
        if (probes === 1) {
          return meResponse({ user: userFixture() });
        }
        throw new ApiProblem(401, "unauthorized", "log in first");
      },
      "/domains": () => {
        throw new ApiProblem(401, "unauthorized", "log in first");
      },
    });

    renderApp("/");

    // The first answer said "Ada"; the refusal says otherwise, and the newer
    // answer is the true one.
    expect(await screen.findByLabelText("Name")).toBeVisible();
  });

  it("leaves the app up when it is the server that went away", async () => {
    let probes = 0;
    serve({
      "/auth/me": () => {
        probes += 1;
        if (probes === 1) {
          return meResponse({ user: userFixture() });
        }
        throw new ApiProblem(503, "unavailable", "restarting");
      },
      "/domains": () => {
        throw new ApiProblem(401, "unauthorized", "log in first");
      },
    });

    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    // The 401 from the sidebar re-probes; the re-probe then fails outright,
    // once and then again after its retry.
    await waitFor(
      () => {
        expect(probes).toBeGreaterThan(2);
      },
      { timeout: 5000 },
    );

    expect(screen.getByRole("heading", { name: "Home" })).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: /cannot reach Crystalline/i }),
    ).not.toBeInTheDocument();
  });
});

describe("a session that expires while the app is open", () => {
  it("sends the browser to the login screen", async () => {
    let signedIn = true;
    serve({
      "/auth/me": () =>
        signedIn ? meResponse({ user: userFixture() }) : meResponse(),
      "/domains": () => {
        if (!signedIn) {
          throw new ApiProblem(401, "unauthorized", "log in first");
        }
        signedIn = false;
        throw new ApiProblem(401, "unauthorized", "log in first");
      },
    });

    renderApp("/");

    await waitFor(
      async () => {
        expect(await screen.findByLabelText("Name")).toBeVisible();
      },
      { timeout: 3000 },
    );
  });
});
