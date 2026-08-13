/**
 * The first visit to an instance nobody has an account on yet.
 *
 * The wizard is the login route wearing a different form, so these tests mount
 * the whole app exactly as the login tests do and let the capability probe
 * decide which form appears. What is pinned here is the set of answers the
 * setup endpoint can give and where each one leaves the person at the keyboard:
 * signed in, holding a token field, holding a refusal with no way forward, or
 * looking at the ordinary login form because somebody else got there first.
 */

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api, setCsrfToken } from "../api/client";
import type { LoginResponse } from "../api/model";
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
const setCsrfTokenMock = vi.mocked(setCsrfToken);

function serve(routes: Record<string, Answer>) {
  apiMock.mockImplementation(answersFor(routes));
}

/** What one request carried, as the app serialized it. */
function sentBody(init?: RequestInit): unknown {
  return JSON.parse(typeof init?.body === "string" ? init.body : "null");
}

/** Every call the app made to the setup endpoint, bodies parsed. */
function setupCalls(): unknown[] {
  return apiMock.mock.calls
    .filter(([path]) => path === "/auth/setup")
    .map(([, init]) => sentBody(init));
}

/** How many times the capability probe was read. */
function probeCount(): number {
  return apiMock.mock.calls.filter(([path]) => path === "/auth/me").length;
}

/** What an element points its `aria-describedby` at, if anything. */
function describedBy(element: HTMLElement): HTMLElement | null {
  const id = element.getAttribute("aria-describedby");
  return id === null ? null : document.getElementById(id);
}

/** Fill the wizard and submit it. */
async function createAdmin(
  name = "ada",
  password = "hunter2",
  confirm = password,
) {
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText("Name"), name);
  await user.type(screen.getByLabelText("Password"), password);
  await user.type(screen.getByLabelText("Confirm password"), confirm);
  await user.click(
    screen.getByRole("button", { name: "Create admin account" }),
  );
  return user;
}

/** The probe answer of an instance with no accounts at all. */
function firstRun() {
  return meResponse({ needs_setup: true });
}

beforeEach(() => {
  apiMock.mockReset();
  setCsrfTokenMock.mockReset();
});

describe("an instance with no accounts yet", () => {
  it("asks for a first admin instead of credentials", async () => {
    serve({ "/auth/me": firstRun });

    renderApp("/login");

    expect(await screen.findByLabelText("Name")).toBeVisible();
    expect(screen.getByLabelText("Password")).toBeVisible();
    expect(screen.getByLabelText("Confirm password")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Create admin account" }),
    ).toBeVisible();
    // This is still the way in to this product, so it still says which
    // product it is.
    expect(screen.getByText("CRYSTALLINE")).toBeVisible();
    // Nobody is asked for a token until the server says it wants one.
    expect(screen.queryByLabelText("Setup token")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Log in" }),
    ).not.toBeInTheDocument();
  });

  it("says why the button is dead before the confirmation is filled in", async () => {
    serve({ "/auth/me": firstRun });

    renderApp("/login");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Name"), "ada");
    await user.type(screen.getByLabelText("Password"), "hunter2");

    // A disabled button suppresses the browser's own "please fill this in"
    // bubble, so a half-filled form that says nothing is a dead end: the
    // password manager that filled one field and not the other leaves people
    // exactly here.
    expect(
      screen.getByText("type the password again to confirm it"),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Create admin account" }),
    ).toBeDisabled();
  });

  it("catches a mistyped confirmation before the server ever hears about it", async () => {
    serve({ "/auth/me": firstRun });

    renderApp("/login");
    await createAdmin("ada", "hunter2", "hunter3");

    expect(await screen.findByText("the passwords do not match")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Create admin account" }),
    ).toBeDisabled();
    expect(setupCalls()).toHaveLength(0);

    // The message announces itself once when it appears; the field has to
    // carry it too, or somebody who tabs back to it hears nothing at all.
    const confirm = screen.getByLabelText("Confirm password");
    expect(confirm).toHaveAttribute("aria-invalid", "true");
    expect(describedBy(confirm)).toHaveTextContent(
      "the passwords do not match",
    );
  });

  it("signs the new admin in and lands them in the app", async () => {
    let created = false;
    serve({
      "/auth/me": () =>
        created
          ? meResponse({ user: userFixture({ role: "admin" }), csrf: "sess" })
          : firstRun(),
      "/auth/setup": (): LoginResponse => {
        created = true;
        return { csrf: "sess", user: userFixture({ role: "admin" }) };
      },
      "/domains": domainsResponse,
    });

    renderApp("/login");
    await createAdmin();

    expect(await screen.findByRole("heading", { name: "Home" })).toBeVisible();
    expect(setCsrfTokenMock).toHaveBeenCalledWith("sess");
    expect(setupCalls()).toEqual([{ name: "ada", password: "hunter2" }]);
  });

  it("asks for the setup token only once the server says it holds one", async () => {
    let created = false;
    serve({
      "/auth/me": () =>
        created
          ? meResponse({ user: userFixture({ role: "admin" }) })
          : firstRun(),
      "/auth/setup": (_path, init): LoginResponse => {
        const body = sentBody(init) as { token?: string };
        if (body.token !== "cafe1234") {
          throw new ApiProblem(
            403,
            "forbidden",
            "this request did not come from the machine that serves this instance",
            { token_required: true },
          );
        }
        created = true;
        return { csrf: "sess", user: userFixture({ role: "admin" }) };
      },
      "/domains": domainsResponse,
    });

    renderApp("/login");
    const user = await createAdmin();

    // The refusal is the server's own words, and it comes with the one field
    // that can answer it.
    expect(
      await screen.findByText(
        "this request did not come from the machine that serves this instance",
      ),
    ).toBeVisible();
    const token = screen.getByLabelText("Setup token");
    expect(token).toBeVisible();

    await user.type(token, "cafe1234");
    await user.click(
      screen.getByRole("button", { name: "Create admin account" }),
    );

    expect(await screen.findByRole("heading", { name: "Home" })).toBeVisible();
    expect(setupCalls()).toEqual([
      { name: "ada", password: "hunter2" },
      { name: "ada", password: "hunter2", token: "cafe1234" },
    ]);
  });

  it("offers no token field when the refusal carries no token to offer", async () => {
    serve({
      "/auth/me": firstRun,
      "/auth/setup": () => {
        throw new ApiProblem(
          403,
          "forbidden",
          "first-run setup is open to this instance's own machine only",
        );
      },
    });

    renderApp("/login");
    await createAdmin();

    expect(
      await screen.findByText(
        "first-run setup is open to this instance's own machine only",
      ),
    ).toBeVisible();
    // A dead-end input is worse than none: this instance has no token, so
    // there is nothing anybody could type here.
    expect(screen.queryByLabelText("Setup token")).not.toBeInTheDocument();
  });

  it("falls back to the login form when someone else got there first", async () => {
    let taken = false;
    serve({
      "/auth/me": () => (taken ? meResponse() : firstRun()),
      "/auth/setup": () => {
        taken = true;
        throw new ApiProblem(
          410,
          "gone",
          "this instance already has an account, so first-run setup is closed: log in instead",
        );
      },
    });

    renderApp("/login");
    await createAdmin();

    // The probe is re-read, so the form collapses to the ordinary login one -
    // carrying the server's own explanation of why it changed under them.
    expect(await screen.findByRole("button", { name: "Log in" })).toBeVisible();
    expect(
      screen.getByText(
        "this instance already has an account, so first-run setup is closed: log in instead",
      ),
    ).toBeVisible();
    await waitFor(() => {
      expect(
        screen.queryByLabelText("Confirm password"),
      ).not.toBeInTheDocument();
    });
    // The sentence lands in the live region that was already on the page.
    expect(screen.getByRole("status")).toHaveTextContent(
      "this instance already has an account, so first-run setup is closed: log in instead",
    );
  });

  it("keeps the live region on the page before it has anything to say", async () => {
    serve({ "/auth/me": firstRun });

    renderApp("/login");

    // A live region that is created together with its text is announced
    // unreliably, and the flip that fills this one also moves focus into the
    // login form. So it waits there, empty, from the first paint.
    await screen.findByLabelText("Confirm password");
    expect(screen.getByRole("status")).toHaveTextContent("");
  });

  it("adds nothing to the login screen of an instance that is set up", async () => {
    serve({ "/auth/me": () => meResponse() });

    renderApp("/login");

    // The wizard's live region belongs to the wizard's own flow: an instance
    // that has accounts renders the card it always rendered.
    expect(await screen.findByRole("button", { name: "Log in" })).toBeVisible();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("does not re-probe the identity when setup itself is refused", async () => {
    serve({
      "/auth/me": firstRun,
      "/auth/setup": () => {
        throw new ApiProblem(401, "unauthorized", "this call was not allowed");
      },
    });

    renderApp("/login");
    await createAdmin();

    await screen.findByText("this call was not allowed");
    // Nobody is signed in yet, so a refusal here is not an expired session:
    // the recovery re-probe would be asking a question already answered.
    expect(probeCount()).toBe(1);
  });

  it("sends one request per submit, whatever the server does with it", async () => {
    serve({
      "/auth/me": firstRun,
      "/auth/setup": () => {
        throw new ApiProblem(
          500,
          "server error",
          "the store could not be read",
        );
      },
    });

    renderApp("/login");
    await createAdmin();

    // Creating an account is not a request to repeat on its own: a retrying
    // pair of mutations would show this message only after four POSTs.
    expect(
      await screen.findByText("the store could not be read"),
    ).toBeVisible();
    expect(setupCalls()).toHaveLength(1);
  });
});
