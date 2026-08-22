/**
 * Sharing from the browser: what a share would do, read before anybody commits
 * to doing it, and the outcome read back in place afterwards.
 *
 * Mounted through the domain screen rather than in isolation, for the same
 * reason the proposals card is: the button that opens this dialog is admin-only
 * and lives on that card, and a dialog tested on its own would pass while
 * nothing on the screen could open it.
 */

import { screen, waitFor, within } from "@testing-library/react";
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

// Above roughly load average 33 this file's slower tests exceed the 5000 ms
// default (a threshold effect measured 2026-08-14, plans history); the raise
// keeps a loaded machine from reading as a failure. Never raise the global
// default to hide this.
vi.setConfig({ testTimeout: 15000 });

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/**
 * The sync status in the shape the real route sends. A team domain with one
 * open proposal, which is what puts the card on the screen; the share button
 * lives in its header either way.
 */
function syncResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    mode: "github",
    repo: "acme/knowledge",
    branch: "main",
    base_commit: "9f3c1a2",
    behind: false,
    local_changes: 1,
    last_checked: "2026-08-21T08:00:00Z",
    probe_error: null,
    connection: { connected: true, user: "octo", token_store: "keychain" },
    open_proposals: [
      {
        number: 4,
        url: "https://github.com/acme/knowledge/pull/4",
        title: "Refine 2 engrams in eng",
        status: "Open",
        review_state: null,
        amended_upstream: false,
        feedback: [],
        updated_at: null,
      },
    ],
    declined_proposals: [],
    conflicts: [],
    ...overrides,
  };
}

function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "# eng\n" }),
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
      "/domains/eng/sync": () => syncResponse(),
      ...routes,
    }),
  );
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/** How many times a route was asked for, exactly. */
function reads(path: string): number {
  return requested().filter((asked) => asked === path).length;
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

/** Open the dialog off the card's header button, once the card is up. */
async function openShareDialog(): Promise<HTMLElement> {
  const card = await screen.findByRole("region", { name: "Proposals" });
  await userEvent.click(
    within(card).getByRole("button", { name: "Share changes" }),
  );
  return screen.findByRole("dialog", { name: /share/i });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the share dialog", () => {
  it("previews the action and shares with an edited title", async () => {
    const shared = vi.fn(() => ({
      outcome: "updated",
      proposal: { number: 4, url: "https://github.com/acme/knowledge/pull/4" },
    }));
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "update",
        number: 4,
        url: "https://github.com/acme/knowledge/pull/4",
        effective_title: "Refine 1 engram in eng",
        changes: [{ path: "notes/a.md", kind: "modified" }],
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST" ? shared() : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();
    const statusReads = reads("/domains/eng/sync");

    // The action line names the update rather than promising a new proposal.
    expect(
      await within(dialog).findByText(/updates proposal #4/i),
    ).toBeInTheDocument();
    // What would go into it, by name: a share nobody can see the contents of
    // is a share nobody can decide about.
    expect(within(dialog).getByText("notes/a.md")).toBeInTheDocument();

    // The title arrives prefilled and editable.
    const title = within(dialog).getByLabelText("Title");
    expect(title).toHaveValue("Refine 1 engram in eng");
    await userEvent.clear(title);
    await userEvent.type(title, "Sharper title");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    await waitFor(() => {
      expect(shared).toHaveBeenCalled();
    });
    // The author's own words reach the server; an untouched description is
    // left out entirely, so the engine writes the body it would have written.
    expect(sentBody("/domains/eng/sync/share", "POST")).toEqual({
      title: "Sharper title",
    });

    // The outcome is surfaced where the decision was made, rather than the
    // dialog closing on nothing.
    expect(
      await within(dialog).findByText(/updated proposal #4/i),
    ).toBeInTheDocument();
    // And the card behind it reads the status again: the proposal it lists
    // just moved.
    await waitFor(() => {
      expect(reads("/domains/eng/sync")).toBeGreaterThan(statusReads);
    });
  });

  it("sends no title when the prefilled one was left alone", async () => {
    const shared = vi.fn(() => ({
      outcome: "proposed",
      number: 7,
      url: "https://github.com/acme/knowledge/pull/7",
    }));
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 2 new engrams from eng",
        changes: [
          { path: "notes/a.md", kind: "added" },
          { path: "notes/b.md", kind: "added" },
        ],
      }),
      "/domains/eng/sync/share": (_path, init) =>
        init?.method === "POST" ? shared() : null,
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    expect(
      await within(dialog).findByText(/opens a new proposal/i),
    ).toBeInTheDocument();
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    await waitFor(() => {
      expect(shared).toHaveBeenCalled();
    });
    // The prefill is what the server would have generated anyway, and sending
    // it back as an explicit title would retitle a proposal nobody asked to
    // rename. An untouched field means "your title", not "this one".
    expect(sentBody("/domains/eng/sync/share", "POST")).toEqual({});
    expect(
      await within(dialog).findByText(/opened proposal #7/i),
    ).toBeInTheDocument();
  });

  it("disables the share with the reason when there is nothing to confirm", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "conflicts_pending",
        count: 2,
        effective_title: "",
        changes: [],
      }),
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeDisabled();
    });
    expect(within(dialog).getByText(/conflicts/i)).toBeInTheDocument();
  });

  it("keeps a refused share in the dialog rather than swallowing it", async () => {
    serve({
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "Share 1 new engram from eng",
        changes: [{ path: "notes/a.md", kind: "added" }],
      }),
      "/domains/eng/sync/share": () => {
        throw new ApiProblem(409, "conflict", "GitHub is not connected");
      },
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Share" }),
      ).toBeEnabled();
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Share" }),
    );

    // The dialog stays open with the refusal on it: the form still holds a
    // title somebody wrote, and closing it would throw that away.
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "GitHub is not connected",
    );
    expect(
      within(dialog).getByRole("button", { name: "Share" }),
    ).toBeInTheDocument();
  });

  it("says what a read-only instance refused rather than showing an empty plan", async () => {
    serve({
      "/domains/eng/sync/changes": () => {
        throw new ApiProblem(
          403,
          "forbidden",
          "this instance is serving read-only",
        );
      },
    });

    renderApp("/d/eng");
    const dialog = await openShareDialog();

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "this instance is serving read-only",
    );
    // Nothing to share into, so nothing that offers to.
    expect(
      within(dialog).getByRole("button", { name: "Share" }),
    ).toBeDisabled();
  });
});
