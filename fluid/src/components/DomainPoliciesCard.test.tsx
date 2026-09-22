/**
 * The domain's policy switches: one row per MANIFEST key the server's registry
 * knows, with what the frontmatter declares beside what holds.
 *
 * Mounted through the domain screen rather than in isolation, the way the two
 * cards beside it are: what a caller may change here is read off the members
 * payload and the capability probe, and a card rendered with hand-made props
 * would pass while nothing on the screen could reach it.
 *
 * The rows are never a list kept in this app. The fixture is the registry's
 * own wire shape, so a key added in core reaches the card without a line here
 * changing - which is the rule this card exists to keep.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Answer } from "../test/harness";
import {
  answersFor,
  defaultPolicyRows,
  domainsResponse,
  manifestSectionsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

vi.setConfig({ testTimeout: 15000 });

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

const MANIFEST = ["# eng", "", "## When to Use", "", "- Route here.", ""].join(
  "\n",
);

/** What one row declares, for a manifest fixture that changes a single key. */
interface Declaration {
  declared: string | null;
  effective: string;
}

/**
 * The manifest payload, with the named rows declared as given and every other
 * row at the registry's own default.
 */
function manifestWith(overrides: Record<string, Declaration>) {
  return {
    domain: "eng",
    markdown: MANIFEST,
    checksum: "abc",
    sections: manifestSectionsResponse({
      policies: defaultPolicyRows().map((row) => ({
        ...row,
        ...(overrides[row.key] ?? {}),
      })),
    }),
  };
}

/** The sync status, which is where the branch a direct share commits to comes from. */
function syncResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    mode: "github",
    repo: "acme/knowledge",
    branch: "main",
    last_checked: "2026-09-22T08:00:00Z",
    local_changes: 0,
    open_proposals: [],
    declined_proposals: [],
    conflicts: [],
    behind: false,
    probe_error: null,
    connection: { connected: true, user: "octo", token_store: "keychain" },
    ...overrides,
  };
}

function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => manifestWith({}),
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/domains/eng/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({
        domain: "eng",
        tags: [],
        categories: [],
        relation_types: [],
      }),
      "/domains/eng/members": () => ({
        owner: null,
        visibility: "shared",
        members: [],
      }),
      "/domains/eng/sync": () => syncResponse(),
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

/** The card itself, once the manifest read behind it has landed. */
async function policiesCard(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: "Domain policies" });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the domain policies card", () => {
  it("draws one row per registry key with its name, declared and effective values and the default marked", async () => {
    serve();

    renderApp("/d/eng");
    const card = await policiesCard();

    expect(
      within(card).getByText("Every switch this MANIFEST can set"),
    ).toBeVisible();
    const sharing = within(card).getByRole("row", { name: /^sharing/ });
    // Nothing in the frontmatter says anything about this key, which is a
    // different sentence from the value it is read as.
    expect(within(sharing).getByText("not declared")).toBeVisible();
    const select = within(card).getByRole("combobox", { name: "sharing" });
    expect(select).toHaveValue("proposal");
    expect(
      within(select).getByRole("option", { name: "proposal (default)" }),
    ).toBeVisible();
    // Every key the registry sent, not the one this feature is about.
    expect(
      within(card).getByRole("combobox", { name: "generated_indexes" }),
    ).toHaveValue("local");
  });

  it("names an unrecognized declaration and what it is read as", async () => {
    serve({
      "/domains/eng/manifest": () =>
        manifestWith({
          sharing: { declared: "dirct", effective: "proposal" },
        }),
    });

    renderApp("/d/eng");
    const card = await policiesCard();

    // A typo in the frontmatter is not an error the server refuses over, so
    // the card says what the domain is actually doing about it.
    expect(
      within(card).getByText("read as proposal; dirct is not a known value"),
    ).toBeVisible();
  });

  it("posts a generated_indexes change at once and takes the answer into the manifest query", async () => {
    const patched = vi.fn(() =>
      manifestWith({
        generated_indexes: { declared: "shared", effective: "shared" },
      }),
    );
    serve({
      "/domains/eng/manifest": (_path, init) =>
        init?.method === "PATCH" ? patched() : manifestWith({}),
    });

    renderApp("/d/eng");
    const card = await policiesCard();

    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "generated_indexes" }),
      "shared",
    );
    await waitFor(() => {
      expect(patched).toHaveBeenCalledTimes(1);
    });
    // One key, the one that moved: the body is a patch rather than a document.
    expect(sentBody("/domains/eng/manifest", "PATCH")).toEqual({
      generated_indexes: "shared",
    });
    // The answer carries the manifest as it now reads, so the declared cell
    // moves without a second read of the route.
    expect(
      await within(card).findByText("shared", { selector: "td" }),
    ).toBeVisible();
  });

  it("arms an inline confirmation for sharing: direct, posts on confirm and reverts with focus on Keep", async () => {
    const patched = vi.fn(() =>
      manifestWith({ sharing: { declared: "direct", effective: "direct" } }),
    );
    serve({
      "/domains/eng/sync": () => syncResponse({ branch: "main" }),
      "/domains/eng/manifest": (_path, init) =>
        init?.method === "PATCH" ? patched() : manifestWith({}),
    });

    renderApp("/d/eng");
    const card = await policiesCard();
    const select = within(card).getByRole("combobox", { name: "sharing" });

    await userEvent.selectOptions(select, "direct");
    // The one key that asks first: the select shows the choice, and nothing
    // has been written yet.
    expect(patched).not.toHaveBeenCalled();
    expect(
      within(card).getByText(
        "Direct sharing removes the review step: every share on this domain commits straight to main, for everybody, until the policy is changed back. Turn it on?",
      ),
    ).toBeVisible();

    await userEvent.click(within(card).getByRole("button", { name: "Keep" }));
    // Back where it was, with the control that asked the question focused.
    expect(select).toHaveValue("proposal");
    expect(select).toHaveFocus();

    await userEvent.selectOptions(select, "direct");
    await userEvent.click(
      within(card).getByRole("button", { name: "Turn on direct sharing" }),
    );
    await waitFor(() => {
      expect(patched).toHaveBeenCalledTimes(1);
    });
    expect(sentBody("/domains/eng/manifest", "PATCH")).toEqual({
      sharing: "direct",
    });
  });

  it("says when the write landed in a draft, and shows a refusal and reverts", async () => {
    serve({
      "/domains/eng/manifest": (_path, init) =>
        init?.method === "PATCH"
          ? {
              ...manifestWith({
                generated_indexes: { declared: "shared", effective: "shared" },
              }),
              draft: true,
            }
          : manifestWith({}),
    });

    const first = renderApp("/d/eng");
    let card = await policiesCard();
    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "generated_indexes" }),
      "shared",
    );

    // A reviewing domain takes this write into the caller's own draft, and
    // the domain's policy is what the draft changes when it lands.
    expect(
      await within(card).findByText(
        "Saved to your draft. The domain's policy changes when the draft is shared and lands.",
      ),
    ).toBeVisible();
    first.unmount();

    serve({
      "/domains/eng/manifest": (_path, init) => {
        if (init?.method === "PATCH") {
          throw new ApiProblem(
            403,
            "forbidden",
            "only the owner of 'eng' or an instance admin may change `generated_indexes`",
          );
        }
        return manifestWith({});
      },
    });

    renderApp("/d/eng");
    card = await policiesCard();
    const select = within(card).getByRole("combobox", {
      name: "generated_indexes",
    });
    await userEvent.selectOptions(select, "shared");

    // The server's own words, and the row back where the server left it.
    expect(await within(card).findByRole("alert")).toHaveTextContent(
      "only the owner of 'eng'",
    );
    expect(select).toHaveValue("local");
  });

  it("disables every select on a read-only instance with the reason, and draws none without the owner right", async () => {
    serve({
      "/auth/me": () =>
        meResponse({ user: userFixture({ role: "admin" }), read_only: true }),
    });

    const first = renderApp("/d/eng");
    let card = await policiesCard();
    const select = within(card).getByRole("combobox", { name: "sharing" });

    // A door that will not open is still shown as the door, with the reason
    // beside it, because this side can prove the refusal.
    expect(select).toBeDisabled();
    expect(select).toHaveAccessibleDescription(
      "This instance is read only, so nothing here can be changed.",
    );
    first.unmount();

    serve({
      "/auth/me": () => meResponse({ user: userFixture({ role: "editor" }) }),
    });

    renderApp("/d/eng");
    card = await policiesCard();

    // A right this side only derived is not shown as a shut door: an account
    // that neither owns the domain nor administers the instance reads the
    // policies and is offered nothing.
    expect(within(card).queryByRole("combobox")).toBeNull();
    expect(within(card).getByRole("row", { name: /^sharing/ })).toBeVisible();
  });
});
