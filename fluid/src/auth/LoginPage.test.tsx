/**
 * The login screen is the one place a person meets an API failure head on, so
 * what the server said has to arrive intact: the problem detail is product
 * copy, written server-side, and is shown word for word rather than
 * paraphrased into a house message that says less.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api, setCsrfToken } from "../api/client";
import type { LoginResponse } from "../api/model";
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

/** How many times the capability probe was read. */
function probeCount(): number {
  return apiMock.mock.calls.filter(([path]) => path === "/auth/me").length;
}

/** Fill both fields and submit. */
async function signIn(name = "ada", password = "hunter2") {
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText("Name"), name);
  await user.type(screen.getByLabelText("Password"), password);
  await user.click(screen.getByRole("button", { name: "Log in" }));
  return user;
}

beforeEach(() => {
  apiMock.mockReset();
  setCsrfTokenMock.mockReset();
});

describe("the login screen", () => {
  it("introduces the app here, and nowhere else", async () => {
    serve({ "/auth/me": () => meResponse() });

    renderApp("/login");

    // The way in is where the app gets to say what it is for; every screen
    // after it belongs to the reader's own work. `Home.test` holds the other
    // half of this: the same line must not reappear there.
    expect(
      await screen.findByText(
        "mind-meld your fluid thoughts with your AI's crystalline intelligence",
      ),
    ).toBeVisible();
  });

  it("names the product and the interface, in that order", async () => {
    serve({ "/auth/me": () => meResponse() });

    renderApp("/login");

    // Which product, then which of its faces. The wordmark is real text
    // rather than the terminal banner's block letters, so it survives being
    // listened to; the heading stays the name of this interface.
    expect(await screen.findByText("CRYSTALLINE")).toBeVisible();
    expect(screen.getByRole("heading", { name: "Fluid" })).toBeVisible();
  });

  it("shows the server's own words when the credentials are refused", async () => {
    serve({
      "/auth/me": () => meResponse(),
      "/auth/login": () => {
        throw new ApiProblem(
          401,
          "unauthorized",
          "the name or password is wrong",
        );
      },
    });

    renderApp("/login");
    await signIn();

    expect(
      await screen.findByText("the name or password is wrong"),
    ).toBeVisible();
  });

  it("does not re-probe the identity when login itself is refused", async () => {
    serve({
      "/auth/me": () => meResponse(),
      "/auth/login": () => {
        throw new ApiProblem(
          401,
          "unauthorized",
          "the name or password is wrong",
        );
      },
    });

    renderApp("/login");
    await signIn();

    await screen.findByText("the name or password is wrong");
    // Nobody had a session to expire: a refused login is not that, and the
    // recovery re-probe would be asking a question already answered.
    expect(probeCount()).toBe(1);
  });

  it("disables the submit button while the attempt is in flight", async () => {
    let release = () => {};
    const pending = new Promise<never>((_resolve, reject) => {
      release = () => {
        reject(
          new ApiProblem(401, "unauthorized", "the name or password is wrong"),
        );
      };
    });
    serve({
      "/auth/me": () => meResponse(),
      "/auth/login": () => pending,
    });

    renderApp("/login");
    await signIn();

    const submit = screen.getByRole("button", { name: "Log in" });
    await waitFor(() => {
      expect(submit).toBeDisabled();
    });

    release();
    await waitFor(() => {
      expect(submit).toBeEnabled();
    });
  });

  it("feeds the session's token to the client and enters the app", async () => {
    let signedIn = false;
    serve({
      "/auth/me": () =>
        signedIn
          ? meResponse({ user: userFixture(), csrf: "sess" })
          : meResponse(),
      "/auth/login": (): LoginResponse => {
        signedIn = true;
        return { csrf: "sess", user: userFixture() };
      },
      "/domains": domainsResponse,
    });

    renderApp("/login");
    await signIn();

    expect(await screen.findByRole("heading", { name: "Home" })).toBeVisible();
    expect(setCsrfTokenMock).toHaveBeenCalledWith("sess");
  });

  it("returns to the screen that sent you here", async () => {
    let signedIn = false;
    serve({
      "/auth/me": () =>
        signedIn ? meResponse({ user: userFixture() }) : meResponse(),
      "/auth/login": (): LoginResponse => {
        signedIn = true;
        return { csrf: "sess", user: userFixture() };
      },
      "/domains": domainsResponse,
      // The domain screen behind the gate reads these; without them it would
      // render its not-found state and this test would be asserting on the
      // wrong screen.
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "" }),
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/vocabulary": () => ({ domain: "eng", tags: [] }),
    });

    // Landing on a domain while signed out bounces to the login screen, which
    // has to remember where the browser was going.
    renderApp("/d/eng");
    await signIn();

    expect(
      await screen.findByRole("heading", { level: 1, name: "eng" }),
    ).toBeVisible();
  });
});
