/**
 * The login screen is the one place a person meets an API failure head on, so
 * what the server said has to arrive intact: the problem detail is product
 * copy, written server-side, and is shown word for word rather than
 * paraphrased into a house message that says less.
 */

import { defaultScheduler, notifyManager } from "@tanstack/react-query";
import { act, screen, waitFor } from "@testing-library/react";
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

  it("offers the provider's own button beside the credentials form", async () => {
    serve({
      "/auth/me": () => meResponse(),
      "/auth/providers": () => ({
        local: true,
        oidc: { enabled: true, name: "Contoso" },
      }),
    });

    renderApp("/login");

    // Labelled with the provider's own name, so somebody told "sign in with
    // Contoso" reads the word they were told. A link rather than a button:
    // what follows is a redirect to the provider's own domain and a redirect
    // back, which a background fetch would walk invisibly.
    const button = await screen.findByRole("link", {
      name: "Sign in with Contoso",
    });
    expect(button).toHaveAttribute("href", "/api/v1/auth/oidc/login");
    // And the local form is still the way in it always was: single sign-on is
    // layered over local accounts, never in place of them.
    expect(screen.getByLabelText("Name")).toBeVisible();
    expect(screen.getByRole("button", { name: "Log in" })).toBeVisible();
  });

  it("draws no provider button on an instance that has none", async () => {
    serve({
      "/auth/me": () => meResponse(),
      "/auth/providers": () => ({ local: true, oidc: { enabled: false } }),
    });

    renderApp("/login");

    await screen.findByLabelText("Name");
    expect(screen.queryByRole("link", { name: /sign in with/i })).toBeNull();
  });

  it("the provider link carries the intended destination as return_to", async () => {
    serve({
      "/auth/me": () => meResponse(),
      "/auth/providers": () => ({
        local: true,
        oidc: { enabled: true, name: "Contoso" },
      }),
    });

    // Not `/login` directly: this is the OAuth consent screen `RequireAuth`
    // intercepted, and the destination it carries along is what the button
    // has to echo as `return_to`, so a provider sign-in started from here
    // comes back to the exact pending authorization rather than the home
    // screen.
    renderApp("/authorize?request=req-1");

    const button = await screen.findByRole("link", {
      name: "Sign in with Contoso",
    });
    expect(button).toHaveAttribute(
      "href",
      "/api/v1/auth/oidc/login?return_to=%2Fauthorize%3Frequest%3Dreq-1",
    );
  });

  it("carries no return_to when nothing redirected here", async () => {
    serve({
      "/auth/me": () => meResponse(),
      "/auth/providers": () => ({
        local: true,
        oidc: { enabled: true, name: "Contoso" },
      }),
    });

    renderApp("/login");

    const button = await screen.findByRole("link", {
      name: "Sign in with Contoso",
    });
    expect(button).toHaveAttribute("href", "/api/v1/auth/oidc/login");
  });

  it("carries no return_to when the interrupted journey was already the home screen", async () => {
    serve({
      // Anonymous stays false: an identity-less request to `/` is exactly
      // what `RequireAuth` intercepts and sends here with `from` set, so
      // this is the "root, but via an interrupted journey" case - distinct
      // from the direct-visit case above, where `from` is unset entirely.
      "/auth/me": () => meResponse(),
      "/auth/providers": () => ({
        local: true,
        oidc: { enabled: true, name: "Contoso" },
      }),
    });

    renderApp("/");

    const button = await screen.findByRole("link", {
      name: "Sign in with Contoso",
    });
    // The destination is `/`, the server's own default, so there is nothing
    // for `return_to` to add - this is the bare href the e2e smoke test
    // pins for a plain sign-in.
    expect(button).toHaveAttribute("href", "/api/v1/auth/oidc/login");
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

  it("does not leave the login screen until the auth context actually knows who you are", async () => {
    let signedIn = false;
    serve({
      "/auth/me": () =>
        signedIn ? meResponse({ user: userFixture() }) : meResponse(),
      "/auth/login": (): LoginResponse => {
        signedIn = true;
        return { csrf: "sess", user: userFixture() };
      },
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "" }),
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/vocabulary": () => ({ domain: "eng", tags: [] }),
    });

    renderApp("/d/eng");
    // Let the initial bounce to /login settle on the real scheduler first -
    // taking it over before the boot probe has delivered would strand that
    // too, and there would be nothing on screen to sign in from.
    await screen.findByLabelText("Name");

    // `notifyManager` (query-core, re-exported from `@tanstack/react-query`)
    // is what carries a query's data from the cache to the React component
    // subscribed to it, and its default scheduler is `setTimeout(fn, 0)` - a
    // real macrotask, one tick behind the plain promise chain a mutation's
    // `onSuccess` runs on. That gap is the whole mechanism this test pins.
    // Holding every callback instead of running it freezes the gap open
    // for as long as the test wants, rather than racing to catch it once.
    const held: Array<() => void> = [];
    notifyManager.setScheduler((callback) => {
      held.push(callback);
    });
    // Held across the whole attempt on purpose: whether THIS node is still
    // the one in the document, rather than merely whether an element with
    // the same role and label is, is what tells a stale bounce apart from
    // never having left. A round trip through the destination and back
    // repaints a screen that looks identical - same heading absent, same
    // button present - but does it by unmounting this login form and
    // mounting a second one, which detaches this exact node. That is the
    // same shape of check `toBeVisible` failed with on the original bug,
    // turned around to name the cause instead of only catching the symptom.
    const nameField = screen.getByLabelText("Name");
    try {
      const user = userEvent.setup();
      await user.type(nameField, "ada");
      await user.type(screen.getByLabelText("Password"), "hunter2");
      await user.click(screen.getByRole("button", { name: "Log in" }));

      // Drain microtasks only. `login()`'s own chain - the login POST,
      // `setCsrfToken`, `invalidateQueries`'s refetch of the probe - is
      // plain async/await with no timer anywhere in it, so by now it has
      // fully resolved; nothing queued through the swapped scheduler has
      // been allowed to run yet.
      for (let i = 0; i < 30; i++) {
        await Promise.resolve();
      }

      // The red this proves: on the code this test ships against, the auth
      // context has not told anyone anything yet, so the browser has not
      // moved from the login screen - this exact form is still mounted.
      // Restore the imperative `navigate(destination, { replace: true })`
      // this task removed from `onSuccess` and this fails here: that call
      // runs on the same microtask chain the loop above just drained, ahead
      // of every scheduler callback, so the route has already changed to
      // the destination and `RequireAuth` has already read the still-stale
      // signed-out `user` there and bounced back - unmounting this form and
      // mounting a second one that looks the same but is not this node.
      expect(nameField.isConnected).toBe(true);
      expect(
        screen.queryByRole("heading", { level: 1, name: "eng" }),
      ).toBeNull();
      expect(
        screen.getByRole("button", { name: "Log in" }),
      ).toBeInTheDocument();

      // Now let every held notification through, including any a delivery
      // schedules in turn, so the me-query's refetched data reaches React.
      await act(async () => {
        while (held.length > 0) {
          const queued = held.splice(0, held.length);
          for (const callback of queued) {
            callback();
          }
          await Promise.resolve();
        }
      });
    } finally {
      // Unconditional: a scheduler that only ever queues and never delivers
      // would silently poison every test that runs after this one in the
      // same worker, so this has to run even if an assertion above throws.
      notifyManager.setScheduler(defaultScheduler);
    }

    expect(
      await screen.findByRole("heading", { level: 1, name: "eng" }),
    ).toBeVisible();
    expect(screen.queryByRole("button", { name: "Log in" })).toBeNull();
  });
});
