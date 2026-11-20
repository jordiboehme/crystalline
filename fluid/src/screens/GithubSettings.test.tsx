/**
 * The GitHub settings screen.
 *
 * The only settings section this app ships, and the one screen where a
 * credential changes hands. What is pinned here is what an admin has to be
 * able to trust: that it exists for nobody else, that it says first how the
 * instance reaches GitHub, that the ways to store a token are the way in
 * without an App and folded away beside one, that Disconnect is offered only
 * where something is stored to remove, that the device flow shows the code and where to type it, that a
 * token typed in is sent once and left nowhere, and that every refusal is the
 * server's own sentence rather than a house message pasted over it.
 */

import type { QueryClient } from "@tanstack/react-query";
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

/**
 * A handle on the data layer the app built for itself.
 *
 * The provider makes its own client per mount, which is what keeps a test's
 * cache from outliving it - so the only way to look inside that cache is to
 * watch it being made. One test needs to: what the mutation cache holds after
 * a token is submitted is the whole point of the shape the screen submits in.
 */
const built = vi.hoisted(() => ({ client: null as QueryClient | null }));

vi.mock("../query/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../query/client")>();
  return {
    ...actual,
    createQueryClient: () => {
      built.client = actual.createQueryClient();
      return built.client;
    },
  };
});

const apiMock = vi.mocked(api);

/** The client the app is running on. */
function queryClient(): QueryClient {
  if (!built.client) {
    throw new Error("the app built no query client");
  }
  return built.client;
}

/** The wire shape of a status, in whichever of its states a test needs. */
function statusPayload(overrides: Record<string, unknown> = {}) {
  return {
    enabled: false,
    connected: false,
    user: null,
    token_store: null,
    pending: null,
    error: null,
    app: null,
    ...overrides,
  };
}

/** The wire shape of an App the instance runs. No secret: there is none. */
const APP = {
  app_id: 7,
  slug: "crystalline-kb",
  source: "keyring",
  client_id: "Iv23.kb",
  client_secret_set: true,
  install_url: "https://github.example/apps/crystalline-kb/installations/new",
  webhook_secret_set: true,
  error: null,
  loading: false,
};

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

    expect(
      await screen.findByText(/nothing is filed at this address/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "GitHub connection" }),
    ).not.toBeInTheDocument();
    // The screen asks the server nothing, and the frame advertises no door
    // this session cannot open.
    expect(settingsCalls()).toHaveLength(0);
    expect(
      screen.queryByRole("link", { name: /github/i }),
    ).not.toBeInTheDocument();
  });

  it("offers the settings door to an admin, inside the identity menu", async () => {
    serveAs("admin");
    renderApp("/settings/github");

    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Ada Lovelace" }),
    );
    const settings = await screen.findByRole("menuitem", {
      name: "GitHub connection",
    });
    expect(settings).toHaveAttribute("href", "/settings/github");
  });

  it("shows the disconnected state with both connect paths", async () => {
    serveAs("admin");
    renderApp("/settings/github");

    expect(
      await screen.findByRole("heading", { name: "GitHub connection" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Not connected.")).toBeInTheDocument();
    // Without an App both ways in stand open, side by side: the browser
    // sign-in as the primary, and a token for an instance nobody is sitting
    // in front of. Nothing is folded, and nothing is stored to disconnect.
    expect(
      screen.getByRole("button", { name: /connect with github/i }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Other ways to connect" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Disconnect" }),
    ).not.toBeInTheDocument();
    expect(screen.getByLabelText(/personal access token/i)).toHaveAttribute(
      "type",
      "password",
    );
    expect(
      screen.getByRole("button", { name: /connect with token/i }),
    ).toBeInTheDocument();
  });

  it("says who opens proposals on the App card, and how to change it", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          app: { app_id: 7, slug: "acme", source: "file" },
          share_identity: "personal",
          share_identity_default: true,
        }),
    });
    renderApp("/settings/github");
    expect(
      await screen.findByText(
        /Each person opens proposals under their own GitHub account.*This is the default with a GitHub App\. To have the App open them, set github.share_identity to instance\./,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /^Scheduled runs go through the App\. Shares from a terminal on the server need the owner's own sign-in: run crystalline connect github --personal there\.$/,
      ),
    ).toBeInTheDocument();
  });

  it("says the App opens proposals in instance mode", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          token_store: "GitHub App",
          app: APP,
          share_identity: "instance",
          share_identity_default: false,
        }),
    });
    renderApp("/settings/github");
    expect(
      await screen.findByText(
        /The App opens proposals and names the person who shares as the commit author\. To have each person open their own, set github.share_identity to personal\./,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/--personal/)).not.toBeInTheDocument();
  });

  it("names the owner's sign-in when personal is set, and promotes no connect for it", async () => {
    // The page's connect stores the instance token, never the owner's own
    // identity, so the way to that identity is the command, and the connect
    // stays folded with the token form.
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          user: "crystalline-kb[bot]",
          token_store: "GitHub App",
          app: APP,
          share_identity: "personal",
          share_identity_default: false,
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(
        /^Scheduled runs and shares from a terminal on the server need the owner's own sign-in: run crystalline connect github --personal there\.$/,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/This is the default/)).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /connect with github/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Other ways to connect" }),
    ).toHaveAttribute("aria-expanded", "false");
  });

  it("puts the App connection first and folds the other ways to connect", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          user: "crystalline-kb[bot]",
          token_store: "GitHub App",
          app: { ...APP, source: "environment" },
          share_identity: "instance",
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(
        "Connected through the GitHub App as crystalline-kb[bot].",
      ),
    ).toBeInTheDocument();
    // Status first: the connection card comes before the App card.
    const headings = screen
      .getAllByRole("heading", { level: 2 })
      .map((heading) => heading.textContent)
      .filter((name) => name === "Connection" || name === "GitHub App");
    expect(headings).toEqual(["Connection", "GitHub App"]);
    // The App's source and secret, in sentences.
    expect(
      screen.getByText("Configured from the environment."),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "People can link their own GitHub account in their profile.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/\(environment\)/)).not.toBeInTheDocument();
    // Nothing is stored for Disconnect to remove, so it is not offered, and
    // the ways to store an instance token are folded.
    expect(
      screen.queryByRole("button", { name: "Disconnect" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /connect with github/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/personal access token/i),
    ).not.toBeInTheDocument();

    const fold = screen.getByRole("button", { name: "Other ways to connect" });
    expect(fold).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(fold);
    expect(fold).toHaveAttribute("aria-expanded", "true");
    expect(
      screen.getByRole("button", { name: /connect with github/i }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/personal access token/i)).toHaveAttribute(
      "type",
      "password",
    );
    expect(screen.getByText(/that token is not used/i)).toBeInTheDocument();
  });

  it("names the App when a token is stored beside it, and offers to remove the token", async () => {
    // The status names the stored token whenever one is on file, but the App
    // carries the work: the card says so and Disconnect removes the token.
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          user: "octo",
          token_store: "keyring",
          app: APP,
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(
        "Connected through the GitHub App crystalline-kb.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "A token for octo is also stored on this instance. It is not used while the App works.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Removes the token stored for octo."),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(
      screen.getByText("The App keeps reading, pulling and sharing."),
    ).toBeInTheDocument();
  });

  it("offers no connect and no Disconnect while the token comes from the environment", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          user: "octo",
          token_store: "environment",
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(
        "Connected as octo, with the token set in CRYSTALLINE_GITHUB_TOKEN on the server.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Disconnect" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /connect with github/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/personal access token/i),
    ).not.toBeInTheDocument();
  });

  it("says nothing is connected while the App is refused", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          app: {
            ...APP,
            install_url: null,
            error: "the private key is not valid",
          },
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(/Not connected: the GitHub App cannot be used/),
    ).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "the private key is not valid",
    );
  });

  it("names a GitHub App plainly, even before GitHub named its bot", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          token_store: "GitHub App",
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText("Connected through the GitHub App."),
    ).toBeInTheDocument();
    expect(screen.queryByText(/did not name/i)).not.toBeInTheDocument();
  });

  it("names the App's bot once GitHub said it", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          user: "crystalline-acme[bot]",
          token_store: "GitHub App",
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(
        "Connected through the GitHub App as crystalline-acme[bot].",
      ),
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
    expect(
      await screen.findByText(
        "Connected as octo, with a token kept in the system keychain.",
      ),
    ).toBeInTheDocument();
  });

  it("leaves no copy of the token in the mutation cache", async () => {
    serveAs("admin", {
      "/settings/github/token": () =>
        statusPayload({
          enabled: true,
          connected: true,
          user: "octo",
          token_store: "keyring",
        }),
    });
    renderApp("/settings/github");

    await userEvent.type(
      await screen.findByLabelText(/personal access token/i),
      "ghp_secret",
    );
    await userEvent.click(
      screen.getByRole("button", { name: /connect with token/i }),
    );
    await waitFor(() => {
      expect(sentBody("/settings/github/token", "POST")).toEqual({
        token: "ghp_secret",
      });
    });

    // The data layer keeps a mutation's variables in its state and never
    // clears them - the entry only leaves on garbage collection, minutes
    // after this screen is gone. So the token is not passed as a variable at
    // all: the request went out, and the cache that recorded it holds a
    // mutation with nothing in it.
    const mutations = queryClient().getMutationCache().getAll();
    expect(mutations.length).toBeGreaterThan(0);
    for (const mutation of mutations) {
      expect(JSON.stringify(mutation.state.variables ?? null)).not.toContain(
        "ghp_secret",
      );
    }
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

    expect(
      await screen.findByText("Removes the token stored for octo."),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(forgotten).not.toHaveBeenCalled();
    expect(
      screen.getByText(/syncing them stops until this instance/i),
    ).toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: /confirm disconnect/i }),
    );
    await waitFor(() => {
      expect(forgotten).toHaveBeenCalled();
    });
    expect(await screen.findByText("Not connected.")).toBeInTheDocument();
    expect(
      screen.getByText(/Connect again to keep team domains in step/),
    ).toBeInTheDocument();
  });

  it("Create GitHub App posts the manifest form", async () => {
    const manifest = { name: "Crystalline kb", public: false };
    serveAs("admin", {
      "/settings/github/app/manifest": () => ({
        post_url:
          "https://github.example/organizations/acme/settings/apps/new?state=s1",
        manifest,
        state: "s1",
      }),
    });
    const sent: HTMLFormElement[] = [];
    const submit = vi
      .spyOn(HTMLFormElement.prototype, "submit")
      .mockImplementation(function (this: HTMLFormElement) {
        sent.push(this);
      });
    renderApp("/settings/github");

    await userEvent.type(await screen.findByLabelText(/organization/i), "acme");
    await userEvent.click(
      screen.getByRole("button", { name: "Create GitHub App" }),
    );

    await waitFor(() => {
      expect(submit).toHaveBeenCalledTimes(1);
    });
    expect(sentBody("/settings/github/app/manifest", "POST")).toEqual({
      org: "acme",
    });
    // A real top-level POST to GitHub's page, carrying the manifest as the
    // one field GitHub reads.
    const [form] = sent;
    expect(form?.action).toBe(
      "https://github.example/organizations/acme/settings/apps/new?state=s1",
    );
    expect(form?.method).toBe("post");
    const field = form?.elements.namedItem("manifest");
    expect(field).toBeInstanceOf(HTMLInputElement);
    expect(JSON.parse((field as HTMLInputElement).value)).toEqual(manifest);
    submit.mockRestore();
  });

  it("sends no organization when the field is left empty", async () => {
    serveAs("admin", {
      "/settings/github/app/manifest": () => {
        throw new ApiProblem(
          409,
          "conflict",
          "creating the GitHub App needs service.public_url",
        );
      },
    });
    renderApp("/settings/github");

    await userEvent.click(
      await screen.findByRole("button", { name: "Create GitHub App" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "creating the GitHub App needs service.public_url",
    );
    expect(sentBody("/settings/github/app/manifest", "POST")).toEqual({});
  });

  it("shows Install on repositories once created", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          enabled: true,
          connected: true,
          token_store: "GitHub App",
          app: APP,
        }),
    });
    renderApp("/settings/github?app=created");

    const install = await screen.findByRole("link", {
      name: "Install on repositories",
    });
    expect(install).toHaveAttribute("href", APP.install_url);
    expect(install).toHaveAttribute("target", "_blank");
    // The landing says what is left to do on GitHub: the manifest cannot
    // switch the device flow on, and the terminal link needs it.
    expect(screen.getByRole("status")).toHaveTextContent(/device flow/i);
    // One App per instance: no second one is offered.
    expect(
      screen.queryByRole("button", { name: "Create GitHub App" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText(/crystalline github app remove/i),
    ).toBeInTheDocument();
  });

  it("says the client secret is set", async () => {
    serveAs("admin", {
      "/settings/github": () => statusPayload({ app: APP }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(
        "People can link their own GitHub account in their profile.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Saved in the system keychain."),
    ).toBeInTheDocument();
    expect(screen.queryByText(/client secret/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/cannot link/i)).not.toBeInTheDocument();
  });

  it.each([
    ["keyring", true],
    ["file", false],
  ])(
    "gives the command to run when a %s App has no client secret",
    async (source, webhook) => {
      serveAs("admin", {
        "/settings/github": () =>
          statusPayload({
            app: {
              ...APP,
              source,
              client_secret_set: false,
              webhook_secret_set: webhook,
            },
          }),
      });
      renderApp("/settings/github");

      expect(
        await screen.findByText(
          /people cannot link their own github account yet, because the app has no client secret/i,
        ),
      ).toBeInTheDocument();
      const command = screen.getByText(/crystalline github app set/);
      expect(command.textContent).toContain("--app-id 7");
      expect(command.textContent).toContain("--client-id Iv23.kb");
      expect(command.textContent).toContain("--private-key-file <file>");
      expect(command.textContent).toContain("--client-secret-file <file>");
      expect(command.textContent?.includes("--webhook-secret-file")).toBe(
        webhook,
      );
      expect(screen.queryByText(/restart/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/CRYSTALLINE_GITHUB_APP/)).toBeNull();
    },
  );

  it("names the variables when an App from the environment has no client secret", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          app: { ...APP, source: "environment", client_secret_set: false },
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(/the App has no client secret/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Set CRYSTALLINE_GITHUB_APP_CLIENT_SECRET or\s+CRYSTALLINE_GITHUB_APP_CLIENT_SECRET_FILE on the server, then restart it\./,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/crystalline github app set/)).toBeNull();
  });

  it("gives only the first sentence when the source is not known", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          app: { ...APP, source: null, client_secret_set: false },
        }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(
        "People cannot link their own GitHub account yet, because the App has no client secret.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/restart/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/crystalline github app set/)).toBeNull();
  });

  it.each(["environment", "keyring", "file"])(
    "claims nothing about the client secret of a refused %s App",
    async (source) => {
      serveAs("admin", {
        "/settings/github": () =>
          statusPayload({
            app: {
              ...APP,
              app_id: null,
              client_id: null,
              slug: null,
              source,
              client_secret_set: false,
              install_url: null,
              error: "the private key is not valid",
            },
          }),
      });
      renderApp("/settings/github");

      expect(
        await screen.findByText("the private key is not valid"),
      ).toBeInTheDocument();
      expect(screen.queryByText(/client secret/i)).not.toBeInTheDocument();
      expect(screen.queryByText(/crystalline github app set/)).toBeNull();
      expect(screen.queryByText(/CRYSTALLINE_GITHUB_APP/)).toBeNull();
    },
  );

  it("shows no client secret line while the App is loading", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          app: {
            ...APP,
            app_id: null,
            source: null,
            client_secret_set: false,
            loading: true,
          },
        }),
    });
    renderApp("/settings/github");

    expect(await screen.findByText("Reading the App")).toBeInTheDocument();
    expect(screen.queryByText(/client secret/i)).not.toBeInTheDocument();
  });

  it("draws no client secret line for a non-admin", async () => {
    serveAs("editor", {
      "/settings/github": () =>
        statusPayload({ app: { ...APP, client_secret_set: false } }),
    });
    renderApp("/settings/github");

    expect(
      await screen.findByText(/nothing is filed at this address/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/client secret/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/cannot link/i)).not.toBeInTheDocument();
  });

  it("offers no Create while a saved App is still loading", async () => {
    serveAs("admin", {
      "/settings/github": () =>
        statusPayload({
          app: {
            app_id: null,
            slug: null,
            source: null,
            client_id: null,
            client_secret_set: false,
            install_url: null,
            webhook_secret_set: false,
            error: null,
            loading: true,
          },
        }),
    });
    renderApp("/settings/github");

    // The status has arrived: the connection line says so.
    expect(await screen.findByText("Not connected.")).toBeInTheDocument();
    expect(screen.getByText("Reading the App")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Create GitHub App" }),
    ).not.toBeInTheDocument();
  });

  it("offers no Create on a read-only instance", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () =>
          meResponse({
            user: userFixture({ name: "root", role: "admin" }),
            read_only: true,
          }),
        "/domains": domainsResponse,
        "/settings/github": () => statusPayload(),
      }),
    );
    renderApp("/settings/github");

    expect(
      await screen.findByRole("heading", { name: "GitHub App" }),
    ).toBeInTheDocument();
    expect(await screen.findByText(/read-only/i)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Create GitHub App" }),
    ).not.toBeInTheDocument();
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
      await screen.findByRole("heading", { name: "GitHub connection" }),
    ).toBeInTheDocument();
  });
});
